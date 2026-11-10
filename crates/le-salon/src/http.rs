//! Adaptateur HTTP minimal pour GPUI (aperçus d'images et vignettes).

use std::time::Duration;

use futures::{AsyncReadExt as _, FutureExt as _, future::BoxFuture};
use gpui::http_client::{
    AsyncBody, HttpClient, Request, Response,
    http::{HeaderValue, Response as HttpResponse},
};

pub struct ReqwestClient {
    client: reqwest::Client,
    user_agent: Option<HeaderValue>,
}

impl ReqwestClient {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(12))
            .redirect(reqwest::redirect::Policy::limited(5))
            .user_agent("Le Salon")
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            client,
            user_agent: HeaderValue::from_str("Le Salon").ok(),
        }
    }
}

impl HttpClient for ReqwestClient {
    fn type_name(&self) -> &'static str {
        "reqwest"
    }

    fn user_agent(&self) -> Option<&HeaderValue> {
        self.user_agent.as_ref()
    }

    fn send(
        &self,
        request: Request<AsyncBody>,
    ) -> BoxFuture<'static, anyhow::Result<Response<AsyncBody>>> {
        let client = self.client.clone();
        async move {
            let (parts, mut body) = request.into_parts();
            let mut bytes = Vec::new();
            body.read_to_end(&mut bytes).await?;

            let mut request = client.request(
                reqwest::Method::from_bytes(parts.method.as_str().as_bytes())?,
                parts.uri.to_string(),
            );
            request = request.headers(parts.headers);
            if !bytes.is_empty() {
                request = request.body(bytes);
            }
            let response = request.send().await?;
            let status = response.status();
            let headers = response.headers().clone();
            let bytes = response.bytes().await?;

            let mut builder = HttpResponse::builder().status(status.as_u16());
            *builder
                .headers_mut()
                .ok_or_else(|| anyhow::anyhow!("invalid response headers"))? = headers;
            Ok(builder.body(AsyncBody::from_bytes(bytes))?)
        }
        .boxed()
    }

    fn proxy(&self) -> Option<&gpui::http_client::Url> {
        None
    }
}
