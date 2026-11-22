//! Explicit native-host support for an HTTP service bound to a loopback origin.
//! This does not relax HTTPS for external clients or other service origins.
use reqwest::{ClientBuilder, Request, Response};
use reqwest_middleware::{Middleware, Next};
use url::{Host, Origin, Url};

pub(super) struct LoopbackHttp {
    origin: Origin,
}

impl LoopbackHttp {
    pub(super) fn for_service(address: &str) -> Option<Self> {
        let url = Url::parse(address).ok()?;
        let loopback = match url.host()? {
            Host::Domain(host) => host.eq_ignore_ascii_case("localhost"),
            Host::Ipv4(host) => host.is_loopback(),
            Host::Ipv6(host) => host.is_loopback(),
        };
        if url.scheme() != "http"
            || !loopback
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return None;
        }
        Some(Self {
            origin: url.origin(),
        })
    }

    pub(super) fn configure(&self, builder: ClientBuilder) -> ClientBuilder {
        // Never forward local service credentials through a proxy or an automatic redirect.
        builder
            .https_only(false)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
    }
}

#[async_trait::async_trait]
impl Middleware for LoopbackHttp {
    async fn handle(
        &self,
        request: Request,
        extensions: &mut http::Extensions,
        next: Next<'_>,
    ) -> reqwest_middleware::Result<Response> {
        let url = request.url();
        if url.origin() != self.origin || !url.username().is_empty() || url.password().is_some() {
            return Err(reqwest_middleware::Error::middleware(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Loopback service request left its configured origin.",
            )));
        }
        next.run(request, extensions).await
    }
}
