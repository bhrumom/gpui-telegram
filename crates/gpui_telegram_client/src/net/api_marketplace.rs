//! HTTP client methods for the live server-published OpenBot Marketplace (`/api/v1/marketplace/*`).

use gpui_telegram_proto::marketplace::{
    CallMarketplaceToolRequest, CallMarketplaceToolResponse, ConnectMarketplaceAppRequest,
    ConnectMarketplaceAppResponse, MarketplaceAppListingDto, MarketplaceCatalogResponse,
    MarketplaceConnectionDto, MarketplaceInstallResponse, PublishMarketplaceAppRequest,
};
use reqwest::Method;

use crate::net::http_client::{HttpClient, HttpClientError};
use crate::net::http_helpers::percent_encode_rfc3986;

impl HttpClient {
    pub async fn get_marketplace_catalog(
        &self,
        token: &str,
    ) -> Result<MarketplaceCatalogResponse, HttpClientError> {
        self.request_json(
            Method::GET,
            "/api/v1/marketplace/catalog",
            Some(token),
            Option::<&()>::None,
        )
        .await
    }

    pub async fn publish_marketplace_app(
        &self,
        token: &str,
        req: &PublishMarketplaceAppRequest,
    ) -> Result<MarketplaceAppListingDto, HttpClientError> {
        self.request_json(Method::POST, "/api/v1/marketplace/apps", Some(token), Some(req))
            .await
    }

    pub async fn list_marketplace_connections(
        &self,
        token: &str,
    ) -> Result<Vec<MarketplaceConnectionDto>, HttpClientError> {
        let val: serde_json::Value = self
            .request_json(
                Method::GET,
                "/api/v1/marketplace/connections",
                Some(token),
                Option::<&()>::None,
            )
            .await?;
        let arr = val
            .get("connections")
            .cloned()
            .unwrap_or_else(|| serde_json::Value::Array(Vec::new()));
        serde_json::from_value(arr).map_err(HttpClientError::Serialization)
    }

    pub async fn connect_marketplace_app(
        &self,
        token: &str,
        req: &ConnectMarketplaceAppRequest,
    ) -> Result<ConnectMarketplaceAppResponse, HttpClientError> {
        self.request_json(
            Method::POST,
            "/api/v1/marketplace/connections/connect",
            Some(token),
            Some(req),
        )
        .await
    }

    pub async fn probe_marketplace_connection(
        &self,
        token: &str,
        app_id: &str,
    ) -> Result<ConnectMarketplaceAppResponse, HttpClientError> {
        let path = format!(
            "/api/v1/marketplace/connections/{}/probe",
            percent_encode_rfc3986(app_id)
        );
        self.request_json(Method::POST, &path, Some(token), Option::<&()>::None)
            .await
    }

    pub async fn call_marketplace_tool(
        &self,
        token: &str,
        app_id: &str,
        req: &CallMarketplaceToolRequest,
    ) -> Result<CallMarketplaceToolResponse, HttpClientError> {
        let path = format!(
            "/api/v1/marketplace/connections/{}/call",
            percent_encode_rfc3986(app_id)
        );
        self.request_json(Method::POST, &path, Some(token), Some(req))
            .await
    }

    pub async fn disconnect_marketplace_app(
        &self,
        token: &str,
        app_id: &str,
    ) -> Result<serde_json::Value, HttpClientError> {
        let path = format!(
            "/api/v1/marketplace/connections/{}",
            percent_encode_rfc3986(app_id)
        );
        self.request_json(Method::DELETE, &path, Some(token), Option::<&()>::None)
            .await
    }

    pub async fn record_marketplace_agent_install(
        &self,
        token: &str,
        slug: &str,
    ) -> Result<MarketplaceInstallResponse, HttpClientError> {
        let path = format!(
            "/api/v1/marketplace/agents/{}/install",
            percent_encode_rfc3986(slug)
        );
        self.request_json(Method::POST, &path, Some(token), Option::<&()>::None)
            .await
    }

    pub async fn record_marketplace_skill_install(
        &self,
        token: &str,
        slug: &str,
    ) -> Result<MarketplaceInstallResponse, HttpClientError> {
        let path = format!(
            "/api/v1/marketplace/skills/{}/install",
            percent_encode_rfc3986(slug)
        );
        self.request_json(Method::POST, &path, Some(token), Option::<&()>::None)
            .await
    }

    pub async fn delete_mcp_server(
        &self,
        token: &str,
        server_id: &str,
    ) -> Result<serde_json::Value, HttpClientError> {
        let path = format!("/api/v1/mcp/servers/{}", percent_encode_rfc3986(server_id));
        self.request_json(Method::DELETE, &path, Some(token), Option::<&()>::None)
            .await
    }
}
