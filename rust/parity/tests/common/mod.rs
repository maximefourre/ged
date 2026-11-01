#![allow(dead_code)]

use parity::forge::Template;
use parity::oauth::{self, Credentials};
use parity::{Api, Backend, backend};

/// A registered client (provider + GED), a genuine token and its claims.
pub struct World {
    pub backend: Box<dyn Backend>,
    pub api: Api,
    pub client: Credentials,
    pub token: String,
    pub template: Template,
}

impl World {
    pub async fn new() -> Self {
        let backend = backend().await;
        let api = Api::new(backend.base_url());
        let client = oauth::register_everywhere(backend.db(), "erp-app").await;
        let token = oauth::fetch_token(&client).await;
        let template = Template::from_token(&token);
        Self {
            backend,
            api,
            client,
            token,
            template,
        }
    }
}
