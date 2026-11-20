use std::{future::Future, sync::OnceLock, time::Duration};

use tokio::sync::watch;

pub(crate) const TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RequestError {
    Cancelled,
    TimedOut,
}

// Only one account authentication request is active at a time. The generation also handles
// cancellation before the worker enters Rust, without keeping a thread busy polling a flag.
static CURRENT_REQUEST: OnceLock<RequestGate> = OnceLock::new();

struct RequestGate(watch::Sender<u64>);

impl RequestGate {
    fn new() -> Self {
        Self(watch::channel(0).0)
    }

    fn begin(&self) -> u64 {
        let mut id = 0;
        self.0.send_modify(|current| {
            *current = current.wrapping_add(1);
            id = *current;
        });
        id
    }

    fn cancel(&self, id: u64) {
        self.0.send_if_modified(|current| {
            if *current != id {
                return false;
            }
            *current = current.wrapping_add(1);
            true
        });
    }

    async fn run<F: Future>(
        &self,
        id: u64,
        request: F,
        timeout: Duration,
    ) -> Result<F::Output, RequestError> {
        let mut receiver = self.0.subscribe();
        if *receiver.borrow_and_update() != id {
            return Err(RequestError::Cancelled);
        }
        tokio::select! {
            biased;
            _ = receiver.changed() => Err(RequestError::Cancelled),
            result = tokio::time::timeout(timeout, request) => {
                result.map_err(|_| RequestError::TimedOut)
            }
        }
    }
}

pub(crate) fn begin() -> u64 {
    CURRENT_REQUEST.get_or_init(RequestGate::new).begin()
}

pub(crate) fn cancel(id: u64) {
    CURRENT_REQUEST.get_or_init(RequestGate::new).cancel(id);
}

pub(crate) fn is_current(id: u64) -> bool {
    *CURRENT_REQUEST.get_or_init(RequestGate::new).0.borrow() == id
}

pub(crate) async fn run<F: Future>(id: u64, request: F) -> Result<F::Output, RequestError> {
    CURRENT_REQUEST
        .get_or_init(RequestGate::new)
        .run(id, request, TIMEOUT)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    #[test]
    fn cancellation_before_worker_start_does_not_run_the_request() {
        let gate = RequestGate::new();
        let id = gate.begin();
        gate.cancel(id);
        let result =
            runtime().block_on(gate.run(id, async { panic!("cancelled work ran") }, TIMEOUT));
        assert_eq!(result, Err(RequestError::Cancelled));
    }

    #[test]
    fn cancellation_interrupts_an_in_flight_request() {
        let gate = RequestGate::new();
        let id = gate.begin();
        let result = runtime().block_on(async {
            let request = gate.run(id, std::future::pending::<()>(), TIMEOUT);
            let cancel = async {
                tokio::task::yield_now().await;
                gate.cancel(id);
            };
            tokio::join!(request, cancel).0
        });
        assert_eq!(result, Err(RequestError::Cancelled));
    }

    #[test]
    fn a_stale_cancel_does_not_cancel_a_newer_request() {
        let gate = RequestGate::new();
        let old = gate.begin();
        let current = gate.begin();
        gate.cancel(old);
        assert_eq!(
            runtime().block_on(gate.run(current, async { 42 }, TIMEOUT)),
            Ok(42)
        );
    }

    #[test]
    fn waiting_for_a_server_has_a_time_limit() {
        let gate = RequestGate::new();
        let id = gate.begin();
        let result = runtime().block_on(gate.run(
            id,
            std::future::pending::<()>(),
            Duration::from_millis(5),
        ));
        assert_eq!(result, Err(RequestError::TimedOut));
    }
}
