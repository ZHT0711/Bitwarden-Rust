use std::{
    collections::{HashMap, VecDeque},
    io::Cursor,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use bytewarden_core::desktop;
use futures::future::{AbortHandle, AbortRegistration, Abortable, Either, select};
use gpui::{BackgroundExecutor, RenderImage};

const MAX_ENTRIES: usize = 128;
const MAX_IN_FLIGHT: usize = 8;
const MAX_IMAGE_DIMENSION: u32 = 256;
const MAX_DECODE_BYTES: u64 = 1024 * 1024;
const ICON_DIMENSION: u32 = 48;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(6);

#[derive(Clone, Debug, PartialEq, Eq)]
struct Scope {
    account_id: String,
    server_mode: u32,
    server_url: String,
}

enum Entry {
    Pending { request_id: u64, abort: AbortHandle },
    Ready(Arc<RenderImage>),
    Miss,
}

#[derive(Default)]
struct State {
    scope: Option<Scope>,
    next_request_id: u64,
    entries: HashMap<String, Entry>,
    lru: VecDeque<String>,
    retired: Vec<Arc<RenderImage>>,
}

impl State {
    fn retire(&mut self, entry: Entry) {
        match entry {
            Entry::Pending { abort, .. } => abort.abort(),
            Entry::Ready(image) => self.retired.push(image),
            Entry::Miss => {}
        }
    }

    fn clear_entries(&mut self) {
        for (_, entry) in std::mem::take(&mut self.entries) {
            self.retire(entry);
        }
        self.lru.clear();
    }

    fn touch(&mut self, host: &str) {
        if self.lru.back().is_some_and(|last| last == host) {
            return;
        }
        if let Some(index) = self.lru.iter().position(|entry| entry == host)
            && let Some(existing) = self.lru.remove(index)
        {
            self.lru.push_back(existing);
            return;
        }
        self.lru.push_back(host.to_owned());
    }

    fn remove_oldest(&mut self) {
        if let Some(host) = self.lru.pop_front()
            && let Some(entry) = self.entries.remove(&host)
        {
            self.retire(entry);
        }
    }

    fn in_flight(&self) -> usize {
        self.entries
            .values()
            .filter(|entry| matches!(entry, Entry::Pending { .. }))
            .count()
    }
}

#[derive(Default)]
struct WorkerState {
    active: usize,
    next_waiter_id: u64,
    waiters: VecDeque<(u64, futures::channel::oneshot::Sender<WorkerPermit>)>,
}

#[derive(Default)]
struct WorkerSlots {
    state: Mutex<WorkerState>,
}

struct WorkerPermit {
    slots: Arc<WorkerSlots>,
    released: bool,
}

impl WorkerSlots {
    async fn acquire(self: &Arc<Self>) -> WorkerPermit {
        loop {
            let (sender, receiver) = futures::channel::oneshot::channel();
            let waiter_id = {
                let mut state = lock(&self.state);
                if state.active < MAX_IN_FLIGHT {
                    state.active += 1;
                    return WorkerPermit {
                        slots: self.clone(),
                        released: false,
                    };
                }
                state.next_waiter_id = state.next_waiter_id.wrapping_add(1);
                let waiter_id = state.next_waiter_id;
                state.waiters.push_back((waiter_id, sender));
                waiter_id
            };
            let mut waiter = WorkerWaiter {
                slots: self.clone(),
                id: waiter_id,
                queued: true,
            };
            if let Ok(permit) = receiver.await {
                waiter.queued = false;
                return permit;
            }
        }
    }
}

async fn with_worker_permit<T, Request, RequestFuture, Timeout, TimeoutFuture>(
    workers: Arc<WorkerSlots>,
    request: Request,
    timeout: Timeout,
) -> Option<T>
where
    Request: FnOnce(WorkerPermit) -> RequestFuture,
    RequestFuture: std::future::Future<Output = Option<T>>,
    Timeout: FnOnce() -> TimeoutFuture,
    TimeoutFuture: std::future::Future<Output = ()>,
{
    let permit = workers.acquire().await;
    match select(Box::pin(request(permit)), Box::pin(timeout())).await {
        Either::Left((result, _)) => result,
        Either::Right(_) => None,
    }
}

struct WorkerWaiter {
    slots: Arc<WorkerSlots>,
    id: u64,
    queued: bool,
}

impl Drop for WorkerWaiter {
    fn drop(&mut self) {
        if self.queued {
            lock(&self.slots.state)
                .waiters
                .retain(|(id, _)| *id != self.id);
        }
    }
}

impl Drop for WorkerPermit {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        let waiter = {
            let mut state = lock(&self.slots.state);
            if state.waiters.is_empty() {
                state.active -= 1;
                None
            } else {
                state.waiters.pop_front()
            }
        };
        self.released = true;
        if let Some((_, sender)) = waiter {
            let permit = WorkerPermit {
                slots: self.slots.clone(),
                released: false,
            };
            if let Err(permit) = sender.send(permit) {
                drop(permit);
            }
        }
    }
}

/// Bounded per-window favicon storage; it never retains a row or item entity.
#[derive(Clone)]
pub struct FaviconCache {
    state: Arc<Mutex<State>>,
    workers: Arc<WorkerSlots>,
}

impl Default for FaviconCache {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(State::default())),
            workers: Arc::new(WorkerSlots::default()),
        }
    }
}

pub enum Lookup {
    Ready(Arc<RenderImage>),
    Miss,
    Pending,
    Deferred,
    Start {
        host: String,
        request_id: u64,
        registration: AbortRegistration,
    },
}

impl FaviconCache {
    /// Keep completed icons across vault snapshots, but never across accounts,
    /// server endpoints, or a locked/signed-out interval.
    pub fn set_scope(&self, account: Option<(&str, u32, &str)>) {
        let mut state = lock(&self.state);
        let scope = account.map(|(account_id, server_mode, server_url)| Scope {
            account_id: account_id.to_owned(),
            server_mode,
            server_url: server_url.to_owned(),
        });
        if state.scope != scope {
            state.clear_entries();
            state.scope = scope;
        }
    }

    pub fn clear(&self) {
        let mut state = lock(&self.state);
        state.clear_entries();
        state.scope = None;
    }

    /// Return evicted images to the UI so it can release their GPU atlas tiles
    /// before the next frame is painted, including after account or icon changes.
    pub fn take_retired(&self) -> Vec<Arc<RenderImage>> {
        std::mem::take(&mut lock(&self.state).retired)
    }

    pub fn lookup(&self, value: Option<&str>) -> Option<Lookup> {
        let host = normalize_hostname(value)?;
        let mut state = lock(&self.state);
        state.scope.as_ref()?;

        if let Some(entry) = state.entries.get(&host) {
            let result = match entry {
                Entry::Pending { .. } => Lookup::Pending,
                Entry::Ready(image) => Lookup::Ready(image.clone()),
                Entry::Miss => Lookup::Miss,
            };
            state.touch(&host);
            return Some(result);
        }

        if state.in_flight() >= MAX_IN_FLIGHT {
            return Some(Lookup::Deferred);
        }
        if state.entries.len() >= MAX_ENTRIES {
            state.remove_oldest();
        }

        state.next_request_id = state.next_request_id.wrapping_add(1);
        let request_id = state.next_request_id;
        let (abort, registration) = AbortHandle::new_pair();
        state
            .entries
            .insert(host.clone(), Entry::Pending { request_id, abort });
        state.touch(&host);
        Some(Lookup::Start {
            host,
            request_id,
            registration,
        })
    }

    /// Complete only the request still registered for this host and service scope.
    /// Late completions after an account/server change are ignored.
    pub fn complete(&self, host: &str, request_id: u64, image: Option<Arc<RenderImage>>) -> bool {
        let mut state = lock(&self.state);
        let matches_request = matches!(
            state.entries.get(host),
            Some(Entry::Pending { request_id: current, .. }) if *current == request_id
        );
        if !matches_request {
            return false;
        }
        state
            .entries
            .insert(host.to_owned(), image.map_or(Entry::Miss, Entry::Ready));
        state.touch(host);
        true
    }

    pub async fn fetch_icon(
        &self,
        executor: BackgroundExecutor,
        host: String,
        server_mode: u32,
        server_url: String,
        registration: AbortRegistration,
    ) -> Option<Arc<RenderImage>> {
        let request = with_worker_permit(
            self.workers.clone(),
            move |permit| async move {
                let (sender, receiver) = futures::channel::oneshot::channel();
                std::thread::Builder::new()
                    .name("bytewarden-website-icon".into())
                    .stack_size(256 * 1024)
                    .spawn(move || {
                        let _permit = permit;
                        let result = desktop::fetch_website_icon(&host, server_mode, &server_url)
                            .ok()
                            .flatten();
                        let _ = sender.send(result);
                    })
                    .ok()?;
                let (mime, bytes) = receiver.await.ok()??;
                if !supported_image_type(&mime) {
                    return None;
                }
                decode_icon(&bytes, &mime)
            },
            || executor.timer(REQUEST_TIMEOUT),
        );
        Abortable::new(request, registration)
            .await
            .unwrap_or_default()
    }

    #[cfg(test)]
    fn entry_count(&self) -> usize {
        lock(&self.state).entries.len()
    }
}

impl Drop for State {
    fn drop(&mut self) {
        self.clear_entries();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Accept only a DNS hostname. Item URI paths, queries, credentials, and schemes
/// cannot be copied into the outbound URL.
pub fn normalize_hostname(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    let value = value.strip_suffix('.').unwrap_or(value);
    if value.is_empty() || value.ends_with('.') || value.len() > 253 || !value.is_ascii() {
        return None;
    }
    let host = value.to_ascii_lowercase();
    if !host.contains('.') {
        return None;
    }
    for label in host.split('.') {
        if label.is_empty()
            || label.len() > 63
            || label.starts_with('-')
            || label.ends_with('-')
            || !label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return None;
        }
    }
    if host.parse::<std::net::IpAddr>().is_ok()
        || host.rsplit_once('.').is_some_and(|(_, suffix)| {
            matches!(
                suffix,
                "onion"
                    | "i2p"
                    | "local"
                    | "localhost"
                    | "internal"
                    | "lan"
                    | "test"
                    | "invalid"
                    | "example"
            )
        })
    {
        return None;
    }
    Some(host)
}

fn supported_image_type(mime: &str) -> bool {
    matches!(
        mime,
        "image/png"
            | "image/jpeg"
            | "image/jpg"
            | "image/webp"
            | "image/gif"
            | "image/bmp"
            | "image/tiff"
            | "image/tif"
    )
}

fn decode_icon(bytes: &[u8], mime: &str) -> Option<Arc<RenderImage>> {
    let expected = match mime {
        "image/png" => image::ImageFormat::Png,
        "image/jpeg" | "image/jpg" => image::ImageFormat::Jpeg,
        "image/webp" => image::ImageFormat::WebP,
        "image/gif" => image::ImageFormat::Gif,
        "image/bmp" => image::ImageFormat::Bmp,
        "image/tiff" | "image/tif" => image::ImageFormat::Tiff,
        _ => return None,
    };
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    if reader.format()? != expected {
        return None;
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    let mut rgba = reader
        .decode()
        .ok()?
        .thumbnail(ICON_DIMENSION, ICON_DIMENSION)
        .into_rgba8();
    // GPUI render buffers use BGRA; preserve the source alpha channel.
    for pixel in rgba.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(rgba)])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostname_normalization_rejects_uri_paths_and_sensitive_components() {
        assert_eq!(
            normalize_hostname(Some("  ExAmPlE.COM. ")).as_deref(),
            Some("example.com")
        );
        assert_eq!(
            normalize_hostname(Some("example.com/path?token=secret")),
            None
        );
        assert_eq!(
            normalize_hostname(Some("https://user:pass@example.com")),
            None
        );
        assert_eq!(normalize_hostname(Some("localhost")), None);
        assert_eq!(normalize_hostname(Some("127.0.0.1")), None);
    }

    #[test]
    fn evicted_and_cleared_images_are_retired_once_for_gpu_release() {
        use std::collections::HashSet;

        let cache = FaviconCache::default();
        cache.set_scope(Some(("account", 0, "")));
        let mut retired_ids = HashSet::new();
        for index in 0..(MAX_ENTRIES * 4) {
            let host = format!("icon-{index}.example.org");
            let Some(Lookup::Start {
                host, request_id, ..
            }) = cache.lookup(Some(&host))
            else {
                panic!("expected a synthetic request")
            };
            let image = Arc::new(RenderImage::new(vec![image::Frame::new(
                image::RgbaImage::new(1, 1),
            )]));
            assert!(cache.complete(&host, request_id, Some(image)));
            for image in cache.take_retired() {
                assert!(retired_ids.insert(image.id));
            }
        }
        assert_eq!(retired_ids.len(), MAX_ENTRIES * 3);
        assert_eq!(cache.entry_count(), MAX_ENTRIES);
        assert!(cache.take_retired().is_empty());

        // An unchanged snapshot retains its live icons.
        cache.set_scope(Some(("account", 0, "")));
        assert!(cache.take_retired().is_empty());
        cache.set_scope(Some(("other-account", 0, "")));
        for image in cache.take_retired() {
            assert!(retired_ids.insert(image.id));
        }
        assert_eq!(retired_ids.len(), MAX_ENTRIES * 4);
        let Some(Lookup::Start {
            host, request_id, ..
        }) = cache.lookup(Some("before-clear.example.org"))
        else {
            panic!("expected a request in the new scope")
        };
        let image = Arc::new(RenderImage::new(vec![image::Frame::new(
            image::RgbaImage::new(1, 1),
        )]));
        let weak = Arc::downgrade(&image);
        assert!(cache.complete(&host, request_id, Some(image)));
        cache.clear();
        let retired = cache.take_retired();
        assert_eq!(retired.len(), 1);
        assert!(weak.upgrade().is_some());
        drop(retired);
        assert!(weak.upgrade().is_none());
        assert!(cache.take_retired().is_empty());
        assert!(cache.lookup(Some("after-clear.example.org")).is_none());
    }

    #[test]
    fn cache_bounds_completed_and_inflight_entries() {
        let cache = FaviconCache::default();
        cache.set_scope(Some(("test-account", 0, "")));

        for index in 0..400 {
            let host = format!("site-{index}.example.com");
            let Some(Lookup::Start {
                host,
                request_id,
                registration: _,
            }) = cache.lookup(Some(&host))
            else {
                panic!("expected a new cached request")
            };
            assert!(cache.complete(&host, request_id, None));
            assert!(cache.entry_count() <= MAX_ENTRIES);
        }
        assert_eq!(cache.entry_count(), MAX_ENTRIES);
        assert!(matches!(
            cache.lookup(Some("site-399.example.com")),
            Some(Lookup::Miss)
        ));
        assert!(matches!(
            cache.lookup(Some("site-0.example.com")),
            Some(Lookup::Start { .. })
        ));

        let other = FaviconCache::default();
        other.set_scope(Some(("test-account", 0, "")));
        for index in 0..MAX_IN_FLIGHT + 8 {
            let host = format!("pending-{index}.example.com");
            let _ = other.lookup(Some(&host));
        }
        let state = lock(&other.state);
        assert_eq!(state.entries.len(), MAX_IN_FLIGHT);
        assert_eq!(state.in_flight(), MAX_IN_FLIGHT);
    }

    #[test]
    fn completed_icons_survive_snapshot_renders_but_not_locked_or_server_changes() {
        let cache = FaviconCache::default();
        let scope = Some(("test-account", 0, ""));
        cache.set_scope(scope);
        let Some(Lookup::Start {
            host,
            request_id,
            registration: _,
        }) = cache.lookup(Some("site.example.org"))
        else {
            panic!("expected initial icon request")
        };
        assert!(cache.complete(&host, request_id, None));

        cache.set_scope(scope);
        assert!(matches!(cache.lookup(Some(&host)), Some(Lookup::Miss)));

        cache.set_scope(Some(("test-account", 2, "https://vault.example.org/")));
        assert_eq!(cache.entry_count(), 0);
        assert!(matches!(
            cache.lookup(Some(&host)),
            Some(Lookup::Start { .. })
        ));

        cache.set_scope(None);
        assert_eq!(cache.entry_count(), 0);
        assert!(cache.lookup(Some(&host)).is_none());
    }

    #[test]
    fn service_change_aborts_old_pending_request_and_rejects_its_completion() {
        let cache = FaviconCache::default();
        cache.set_scope(Some(("account-a", 0, "")));
        let Some(Lookup::Start {
            host,
            request_id,
            registration: _,
        }) = cache.lookup(Some("site.example.org"))
        else {
            panic!("expected initial icon request")
        };

        cache.set_scope(Some(("account-a", 2, "https://vault.example.org/")));
        assert!(!cache.complete(&host, request_id, None));
        let Some(Lookup::Start {
            host: new_host,
            request_id: new_id,
            registration: _,
        }) = cache.lookup(Some(&host))
        else {
            panic!("changed service should start a fresh request")
        };
        assert_eq!(new_host, host);
        assert_ne!(new_id, request_id);
    }

    #[test]
    fn clear_aborts_pending_icon_work_and_rejects_late_completion() {
        use std::{future::Future, task::Poll};

        let cache = FaviconCache::default();
        cache.set_scope(Some(("account-a", 0, "")));
        let Some(Lookup::Start {
            host,
            request_id,
            registration,
        }) = cache.lookup(Some("site.example.org"))
        else {
            panic!("expected initial icon request")
        };
        let mut request = Box::pin(Abortable::new(
            futures::future::pending::<()>(),
            registration,
        ));
        let mut context = std::task::Context::from_waker(futures::task::noop_waker_ref());
        assert!(request.as_mut().poll(&mut context).is_pending());

        cache.clear();

        assert!(matches!(
            request.as_mut().poll(&mut context),
            Poll::Ready(Err(_))
        ));
        assert_eq!(cache.entry_count(), 0);
        assert!(cache.lookup(Some(&host)).is_none());
        assert!(!cache.complete(&host, request_id, None));
    }

    #[test]
    fn native_worker_permits_bound_concurrency_and_handoff() {
        use std::{future::Future, task::Poll};

        let slots = Arc::new(WorkerSlots::default());
        let mut permits = (0..MAX_IN_FLIGHT)
            .map(|_| futures::executor::block_on(slots.acquire()))
            .collect::<Vec<_>>();
        assert_eq!(lock(&slots.state).active, MAX_IN_FLIGHT);

        let mut first = Box::pin(slots.acquire());
        let mut second = Box::pin(slots.acquire());
        let mut context = std::task::Context::from_waker(futures::task::noop_waker_ref());
        assert!(first.as_mut().poll(&mut context).is_pending());
        assert!(second.as_mut().poll(&mut context).is_pending());
        assert_eq!(lock(&slots.state).waiters.len(), 2);

        drop(permits.pop());
        let first_permit = match first.as_mut().poll(&mut context) {
            Poll::Ready(permit) => permit,
            Poll::Pending => panic!("first waiter receives released slot"),
        };
        assert!(second.as_mut().poll(&mut context).is_pending());
        drop(first_permit);
        let second_permit = match second.as_mut().poll(&mut context) {
            Poll::Ready(permit) => permit,
            Poll::Pending => panic!("second waiter receives next released slot"),
        };
        drop(second_permit);
        drop(permits);
        assert_eq!(lock(&slots.state).active, 0);
        assert!(lock(&slots.state).waiters.is_empty());
    }

    #[test]
    fn queued_time_does_not_consume_the_native_request_timeout() {
        use std::{
            future::Future,
            sync::atomic::{AtomicBool, Ordering},
            task::Poll,
        };

        let slots = Arc::new(WorkerSlots::default());
        let mut permits = (0..MAX_IN_FLIGHT)
            .map(|_| futures::executor::block_on(slots.acquire()))
            .collect::<Vec<_>>();
        let request_started = Arc::new(AtomicBool::new(false));
        let timer_started = Arc::new(AtomicBool::new(false));
        let request_flag = request_started.clone();
        let timer_flag = timer_started.clone();
        let (timeout_sender, timeout_receiver) = futures::channel::oneshot::channel::<()>();
        let mut request = Box::pin(with_worker_permit(
            slots.clone(),
            move |permit| async move {
                let _permit = permit;
                request_flag.store(true, Ordering::SeqCst);
                futures::future::pending::<Option<()>>().await
            },
            move || async move {
                timer_flag.store(true, Ordering::SeqCst);
                let _ = timeout_receiver.await;
            },
        ));
        let mut context = std::task::Context::from_waker(futures::task::noop_waker_ref());
        assert!(request.as_mut().poll(&mut context).is_pending());
        assert!(!timer_started.load(Ordering::SeqCst));
        assert!(!request_started.load(Ordering::SeqCst));

        drop(permits.pop());
        assert!(request.as_mut().poll(&mut context).is_pending());
        assert!(timer_started.load(Ordering::SeqCst));
        assert!(request_started.load(Ordering::SeqCst));
        assert_eq!(lock(&slots.state).active, MAX_IN_FLIGHT);

        assert!(timeout_sender.send(()).is_ok());
        assert!(matches!(
            request.as_mut().poll(&mut context),
            Poll::Ready(None)
        ));
        assert_eq!(lock(&slots.state).active, MAX_IN_FLIGHT - 1);
        drop(permits);
        assert_eq!(lock(&slots.state).active, 0);
    }

    #[test]
    fn canceling_a_queued_native_worker_removes_its_waiter() {
        use std::future::Future;

        let slots = Arc::new(WorkerSlots::default());
        let permits = (0..MAX_IN_FLIGHT)
            .map(|_| futures::executor::block_on(slots.acquire()))
            .collect::<Vec<_>>();
        let mut waiting = Box::pin(slots.acquire());
        let mut context = std::task::Context::from_waker(futures::task::noop_waker_ref());
        assert!(waiting.as_mut().poll(&mut context).is_pending());
        assert_eq!(lock(&slots.state).waiters.len(), 1);
        drop(waiting);
        assert!(lock(&slots.state).waiters.is_empty());
        drop(permits);
        assert_eq!(lock(&slots.state).active, 0);
    }

    #[test]
    fn cache_scope_changes_do_not_release_running_native_worker_slots() {
        use std::{future::Future, task::Poll};

        let cache = FaviconCache::default();
        let clone = cache.clone();
        let mut running = (0..MAX_IN_FLIGHT)
            .map(|_| futures::executor::block_on(cache.workers.acquire()))
            .collect::<Vec<_>>();
        cache.set_scope(Some(("account-a", 0, "")));
        cache.set_scope(Some(("account-b", 0, "")));
        cache.clear();
        assert_eq!(lock(&cache.workers.state).active, MAX_IN_FLIGHT);

        let mut next = Box::pin(clone.workers.acquire());
        let mut context = std::task::Context::from_waker(futures::task::noop_waker_ref());
        assert!(next.as_mut().poll(&mut context).is_pending());
        drop(running.pop());
        let next_permit = match next.as_mut().poll(&mut context) {
            Poll::Ready(permit) => permit,
            Poll::Pending => panic!("worker slot must be released only when native work ends"),
        };
        drop(next_permit);
        drop(running);
        assert_eq!(lock(&cache.workers.state).active, 0);
    }

    #[test]
    fn transparent_png_fixture_keeps_zero_alpha_in_render_image() {
        let mut png = Cursor::new(Vec::new());
        let source = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            1,
            1,
            image::Rgba([24, 48, 96, 0]),
        ));
        assert!(source.write_to(&mut png, image::ImageFormat::Png).is_ok());
        let rendered = decode_icon(png.get_ref(), "image/png").expect("transparent PNG decodes");
        assert_eq!(rendered.size(0).width.0, ICON_DIMENSION as i32);
        assert_eq!(rendered.size(0).height.0, ICON_DIMENSION as i32);
        assert_eq!(rendered.as_bytes(0).map(|bytes| bytes[3]), Some(0));
    }
}
