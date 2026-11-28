use crate::client::TelegramClient;
use crate::net::HttpClientError;

impl TelegramClient {
    pub async fn list_tasks_with_token(
        &self,
        token: &str,
        expected_epoch: u64,
    ) -> std::result::Result<Vec<serde_json::Value>, HttpClientError> {
        self.ensure_pinned_session(expected_epoch, token)?;
        self.http_client.list_tasks(token).await
    }

    pub fn list_tasks(
        &self,
    ) -> impl std::future::Future<Output = std::result::Result<Vec<serde_json::Value>, HttpClientError>>
           + Send
           + '_ {
        let pinned = self.snapshot_pinned_session();
        async move {
            let (expected_epoch, token) = pinned?;
            self.list_tasks_with_token(&token, expected_epoch).await
        }
    }

    pub async fn create_task_with_token(
        &self,
        token: &str,
        expected_epoch: u64,
        payload: &serde_json::Value,
    ) -> std::result::Result<serde_json::Value, HttpClientError> {
        self.ensure_pinned_session(expected_epoch, token)?;
        self.http_client.create_task(token, payload).await
    }

    pub fn create_task<'a>(
        &'a self,
        payload: &'a serde_json::Value,
    ) -> impl std::future::Future<Output = std::result::Result<serde_json::Value, HttpClientError>>
           + Send
           + 'a {
        let pinned = self.snapshot_pinned_session();
        async move {
            let (expected_epoch, token) = pinned?;
            self.create_task_with_token(&token, expected_epoch, payload)
                .await
        }
    }

    pub async fn approve_task_with_token(
        &self,
        token: &str,
        expected_epoch: u64,
        task_id: &str,
        approved: bool,
    ) -> std::result::Result<serde_json::Value, HttpClientError> {
        self.ensure_pinned_session(expected_epoch, token)?;
        self.http_client.approve_task(token, task_id, approved).await
    }

    pub fn approve_task<'a>(
        &'a self,
        task_id: &'a str,
        approved: bool,
    ) -> impl std::future::Future<Output = std::result::Result<serde_json::Value, HttpClientError>>
           + Send
           + 'a {
        let pinned = self.snapshot_pinned_session();
        async move {
            let (expected_epoch, token) = pinned?;
            self.approve_task_with_token(&token, expected_epoch, task_id, approved)
                .await
        }
    }

    pub async fn list_mcp_servers_with_token(
        &self,
        token: &str,
        expected_epoch: u64,
    ) -> std::result::Result<Vec<serde_json::Value>, HttpClientError> {
        self.ensure_pinned_session(expected_epoch, token)?;
        self.http_client.list_mcp_servers(token).await
    }

    pub fn list_mcp_servers(
        &self,
    ) -> impl std::future::Future<Output = std::result::Result<Vec<serde_json::Value>, HttpClientError>>
           + Send
           + '_ {
        let pinned = self.snapshot_pinned_session();
        async move {
            let (expected_epoch, token) = pinned?;
            self.list_mcp_servers_with_token(&token, expected_epoch)
                .await
        }
    }

    pub fn register_mcp_server<'a>(
        &'a self,
        payload: &'a serde_json::Value,
    ) -> impl std::future::Future<Output = std::result::Result<serde_json::Value, HttpClientError>>
           + Send
           + 'a {
        let pinned = self.snapshot_pinned_session();
        async move {
            let (expected_epoch, token) = pinned?;
            self.ensure_pinned_session(expected_epoch, &token)?;
            self.http_client.register_mcp_server(&token, payload).await
        }
    }

    pub async fn list_mcp_tools_with_token(
        &self,
        token: &str,
        expected_epoch: u64,
    ) -> std::result::Result<Vec<serde_json::Value>, HttpClientError> {
        self.ensure_pinned_session(expected_epoch, token)?;
        self.http_client.list_mcp_tools(token).await
    }

    pub async fn get_marketplace_catalog_with_token(
        &self,
        token: &str,
        expected_epoch: u64,
    ) -> std::result::Result<gpui_telegram_proto::MarketplaceCatalogResponse, HttpClientError> {
        self.ensure_pinned_session(expected_epoch, token)?;
        self.http_client.get_marketplace_catalog(token).await
    }

    pub async fn publish_marketplace_app_with_token(
        &self,
        token: &str,
        expected_epoch: u64,
        req: &gpui_telegram_proto::PublishMarketplaceAppRequest,
    ) -> std::result::Result<gpui_telegram_proto::MarketplaceAppListingDto, HttpClientError> {
        self.ensure_pinned_session(expected_epoch, token)?;
        self.http_client.publish_marketplace_app(token, req).await
    }

    pub async fn connect_marketplace_app_with_token(
        &self,
        token: &str,
        expected_epoch: u64,
        req: &gpui_telegram_proto::ConnectMarketplaceAppRequest,
    ) -> std::result::Result<gpui_telegram_proto::ConnectMarketplaceAppResponse, HttpClientError> {
        self.ensure_pinned_session(expected_epoch, token)?;
        self.http_client.connect_marketplace_app(token, req).await
    }

    pub async fn probe_marketplace_connection_with_token(
        &self,
        token: &str,
        expected_epoch: u64,
        app_id: &str,
    ) -> std::result::Result<gpui_telegram_proto::ConnectMarketplaceAppResponse, HttpClientError> {
        self.ensure_pinned_session(expected_epoch, token)?;
        self.http_client
            .probe_marketplace_connection(token, app_id)
            .await
    }

    pub async fn call_marketplace_tool_with_token(
        &self,
        token: &str,
        expected_epoch: u64,
        app_id: &str,
        req: &gpui_telegram_proto::CallMarketplaceToolRequest,
    ) -> std::result::Result<gpui_telegram_proto::CallMarketplaceToolResponse, HttpClientError> {
        self.ensure_pinned_session(expected_epoch, token)?;
        self.http_client
            .call_marketplace_tool(token, app_id, req)
            .await
    }

    pub async fn disconnect_marketplace_app_with_token(
        &self,
        token: &str,
        expected_epoch: u64,
        app_id: &str,
    ) -> std::result::Result<serde_json::Value, HttpClientError> {
        self.ensure_pinned_session(expected_epoch, token)?;
        self.http_client
            .disconnect_marketplace_app(token, app_id)
            .await
    }

    pub async fn record_marketplace_agent_install_with_token(
        &self,
        token: &str,
        expected_epoch: u64,
        slug: &str,
    ) -> std::result::Result<gpui_telegram_proto::MarketplaceInstallResponse, HttpClientError> {
        self.ensure_pinned_session(expected_epoch, token)?;
        self.http_client
            .record_marketplace_agent_install(token, slug)
            .await
    }

    pub async fn record_marketplace_skill_install_with_token(
        &self,
        token: &str,
        expected_epoch: u64,
        slug: &str,
    ) -> std::result::Result<gpui_telegram_proto::MarketplaceInstallResponse, HttpClientError> {
        self.ensure_pinned_session(expected_epoch, token)?;
        self.http_client
            .record_marketplace_skill_install(token, slug)
            .await
    }

    pub async fn delete_mcp_server_with_token(
        &self,
        token: &str,
        expected_epoch: u64,
        server_id: &str,
    ) -> std::result::Result<serde_json::Value, HttpClientError> {
        self.ensure_pinned_session(expected_epoch, token)?;
        self.http_client.delete_mcp_server(token, server_id).await
    }

    pub fn list_mcp_tools(
        &self,
    ) -> impl std::future::Future<Output = std::result::Result<Vec<serde_json::Value>, HttpClientError>>
           + Send
           + '_ {
        let pinned = self.snapshot_pinned_session();
        async move {
            let (expected_epoch, token) = pinned?;
            self.list_mcp_tools_with_token(&token, expected_epoch).await
        }
    }

    pub async fn call_mcp_tool_with_token(
        &self,
        token: &str,
        expected_epoch: u64,
        payload: &serde_json::Value,
    ) -> std::result::Result<serde_json::Value, HttpClientError> {
        self.ensure_pinned_session(expected_epoch, token)?;
        self.http_client.call_mcp_tool(token, payload).await
    }

    pub fn call_mcp_tool<'a>(
        &'a self,
        payload: &'a serde_json::Value,
    ) -> impl std::future::Future<Output = std::result::Result<serde_json::Value, HttpClientError>>
           + Send
           + 'a {
        let pinned = self.snapshot_pinned_session();
        async move {
            let (expected_epoch, token) = pinned?;
            self.call_mcp_tool_with_token(&token, expected_epoch, payload)
                .await
        }
    }

    pub async fn approve_mcp_permission_with_token(
        &self,
        token: &str,
        expected_epoch: u64,
        payload: &serde_json::Value,
    ) -> std::result::Result<serde_json::Value, HttpClientError> {
        self.ensure_pinned_session(expected_epoch, token)?;
        self.http_client.approve_mcp_permission(token, payload).await
    }

    pub fn approve_mcp_permission<'a>(
        &'a self,
        payload: &'a serde_json::Value,
    ) -> impl std::future::Future<Output = std::result::Result<serde_json::Value, HttpClientError>>
           + Send
           + 'a {
        let pinned = self.snapshot_pinned_session();
        async move {
            let (expected_epoch, token) = pinned?;
            self.approve_mcp_permission_with_token(&token, expected_epoch, payload)
                .await
        }
    }

    pub fn acquire_computer_lease<'a>(
        &'a self,
        payload: &'a serde_json::Value,
    ) -> impl std::future::Future<Output = std::result::Result<serde_json::Value, HttpClientError>>
           + Send
           + 'a {
        let pinned = self.snapshot_pinned_session();
        async move {
            let (expected_epoch, token) = pinned?;
            self.ensure_pinned_session(expected_epoch, &token)?;
            self.http_client.acquire_computer_lease(&token, payload).await
        }
    }

    pub fn execute_computer_action<'a>(
        &'a self,
        payload: &'a serde_json::Value,
    ) -> impl std::future::Future<Output = std::result::Result<serde_json::Value, HttpClientError>>
           + Send
           + 'a {
        let pinned = self.snapshot_pinned_session();
        async move {
            let (expected_epoch, token) = pinned?;
            self.ensure_pinned_session(expected_epoch, &token)?;
            self.http_client
                .execute_computer_action(&token, payload)
                .await
        }
    }

    pub fn create_talk_session<'a>(
        &'a self,
        payload: &'a serde_json::Value,
    ) -> impl std::future::Future<Output = std::result::Result<serde_json::Value, HttpClientError>>
           + Send
           + 'a {
        let pinned = self.snapshot_pinned_session();
        async move {
            let (expected_epoch, token) = pinned?;
            self.ensure_pinned_session(expected_epoch, &token)?;
            self.http_client.create_talk_session(&token, payload).await
        }
    }

    pub fn talk_barge_in<'a>(
        &'a self,
        payload: &'a serde_json::Value,
    ) -> impl std::future::Future<Output = std::result::Result<serde_json::Value, HttpClientError>>
           + Send
           + 'a {
        let pinned = self.snapshot_pinned_session();
        async move {
            let (expected_epoch, token) = pinned?;
            self.ensure_pinned_session(expected_epoch, &token)?;
            self.http_client.talk_barge_in(&token, payload).await
        }
    }

    pub fn enqueue_talk_audio<'a>(
        &'a self,
        payload: &'a serde_json::Value,
    ) -> impl std::future::Future<Output = std::result::Result<serde_json::Value, HttpClientError>>
           + Send
           + 'a {
        let pinned = self.snapshot_pinned_session();
        async move {
            let (expected_epoch, token) = pinned?;
            self.ensure_pinned_session(expected_epoch, &token)?;
            self.http_client.enqueue_talk_audio(&token, payload).await
        }
    }
}
