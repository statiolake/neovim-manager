use anyhow::Result;
use chrono::Utc;
use log::{error, info};
use neovim_manager::{
    errors, utils, HealthStatus, InstanceInfo, InstanceResult, InstanceStorage, JsonRpcError,
    JsonRpcRequest, JsonRpcResponse, QueryInstanceParams, RegisterInstanceParams,
    UnregisterInstanceParams, DEFAULT_BIND_ADDR, DEFAULT_PORT,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;

type SharedInstanceStorage = Arc<RwLock<InstanceStorage>>;

struct InstanceManager {
    instances: SharedInstanceStorage,
}

impl InstanceManager {
    fn new() -> Self {
        Self {
            instances: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    async fn health_check_all(&self) -> Result<()> {
        let mut instances = self.instances.write().await;
        let now = Utc::now();
        let mut to_remove = Vec::new();

        for (identifier, instance) in instances.iter_mut() {
            let is_healthy = utils::check_nvim_instance(&instance.server_address).unwrap_or(false);
            instance.last_health_check = now;

            if is_healthy {
                if matches!(instance.health_status, HealthStatus::Unknown) {
                    info!("Instance {identifier} is now healthy");
                }
                instance.health_status = HealthStatus::Healthy;
                instance.last_ping = now;
            } else {
                // ヘルスチェック失敗 = プロセス終了なので即座に削除
                info!("Instance {identifier} is no longer responding, removing");
                to_remove.push(identifier.clone());
            }
        }

        for identifier in to_remove {
            instances.remove(&identifier);
            info!("Removed unresponsive instance: {identifier}");
        }

        Ok(())
    }

    async fn query_instance(&self, identifier: &str) -> Result<Option<InstanceResult>> {
        // ヘルスチェックは別途実行するので、クエリ時は実行しない
        // self.health_check_all().await?;

        let instances = self.instances.read().await;
        if let Some(instance) = instances.get(identifier) {
            Ok(Some(InstanceResult {
                identifier: instance.identifier.clone(),
                server_address: instance.server_address.clone(),
                health_status: instance.health_status.clone(),
                last_health_check: instance.last_health_check,
            }))
        } else {
            Ok(None)
        }
    }

    async fn list_instances(&self) -> Result<Vec<InstanceResult>> {
        self.health_check_all().await?;

        let instances = self.instances.read().await;
        let results = instances
            .values()
            .map(|instance| InstanceResult {
                identifier: instance.identifier.clone(),
                server_address: instance.server_address.clone(),
                health_status: instance.health_status.clone(),
                last_health_check: instance.last_health_check,
            })
            .collect();

        Ok(results)
    }

    async fn register_instance(&self, identifier: String, server_address: String) -> Result<()> {
        let mut instances = self.instances.write().await;

        if instances.contains_key(&identifier) {
            return Err(anyhow::anyhow!("Instance already exists"));
        }

        let instance = InstanceInfo {
            identifier: identifier.clone(),
            server_address,
            registered_at: Utc::now(),
            last_ping: Utc::now(),
            health_status: HealthStatus::Unknown,
            last_health_check: Utc::now(),
        };

        instances.insert(identifier.clone(), instance);
        info!("Registered instance: {identifier}");

        Ok(())
    }

    async fn unregister_instance(&self, identifier: &str) -> Result<()> {
        let mut instances = self.instances.write().await;

        if instances.remove(identifier).is_some() {
            info!("Unregistered instance: {identifier}");
            Ok(())
        } else {
            Err(anyhow::anyhow!("Instance not found"))
        }
    }

    async fn handle_request(&self, request: JsonRpcRequest) -> JsonRpcResponse {
        let id = request.id.clone();

        let result = match request.method.as_str() {
            "query_instance" => {
                match serde_json::from_value::<QueryInstanceParams>(request.params) {
                    Ok(params) => match self.query_instance(&params.identifier).await {
                        Ok(Some(instance)) => Ok(json!(instance)),
                        Ok(None) => Ok(Value::Null),
                        Err(e) => Err(JsonRpcError {
                            code: errors::INTERNAL_ERROR,
                            message: e.to_string(),
                            data: None,
                        }),
                    },
                    Err(e) => Err(JsonRpcError {
                        code: errors::INTERNAL_ERROR,
                        message: format!("Invalid parameters: {e}"),
                        data: None,
                    }),
                }
            }
            "list_instances" => match self.list_instances().await {
                Ok(instances) => Ok(json!(instances)),
                Err(e) => Err(JsonRpcError {
                    code: errors::INTERNAL_ERROR,
                    message: e.to_string(),
                    data: None,
                }),
            },
            "register_instance" => {
                match serde_json::from_value::<RegisterInstanceParams>(request.params) {
                    Ok(params) => {
                        match self
                            .register_instance(params.identifier.clone(), params.server_address)
                            .await
                        {
                            Ok(()) => Ok(json!("registered")),
                            Err(_) => Err(JsonRpcError {
                                code: errors::INSTANCE_ALREADY_EXISTS,
                                message: "Instance already exists".to_string(),
                                data: Some(json!({"identifier": params.identifier})),
                            }),
                        }
                    }
                    Err(e) => Err(JsonRpcError {
                        code: errors::INTERNAL_ERROR,
                        message: format!("Invalid parameters: {e}"),
                        data: None,
                    }),
                }
            }
            "unregister_instance" => {
                match serde_json::from_value::<UnregisterInstanceParams>(request.params) {
                    Ok(params) => match self.unregister_instance(&params.identifier).await {
                        Ok(()) => Ok(json!("unregistered")),
                        Err(_) => Err(JsonRpcError {
                            code: errors::INSTANCE_NOT_FOUND,
                            message: "Instance not found".to_string(),
                            data: Some(json!({"identifier": params.identifier})),
                        }),
                    },
                    Err(e) => Err(JsonRpcError {
                        code: errors::INTERNAL_ERROR,
                        message: format!("Invalid parameters: {e}"),
                        data: None,
                    }),
                }
            }
            "shutdown" => {
                info!("Shutdown requested");
                std::process::exit(0);
            }
            _ => Err(JsonRpcError {
                code: -32601,
                message: "Method not found".to_string(),
                data: None,
            }),
        };

        match result {
            Ok(result) => JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                result: Some(result),
                error: None,
                id,
            },
            Err(error) => JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                result: None,
                error: Some(error),
                id,
            },
        }
    }
}

async fn handle_client(stream: TcpStream, manager: Arc<InstanceManager>) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    loop {
        line.clear();
        let bytes_read = reader.read_line(&mut line).await?;

        if bytes_read == 0 {
            // Client disconnected
            break;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        info!("Received request: {trimmed}");

        let response = match serde_json::from_str::<JsonRpcRequest>(trimmed) {
            Ok(request) => manager.handle_request(request).await,
            Err(e) => {
                error!("Failed to parse JSON-RPC request: {e}");
                JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32700,
                        message: "Parse error".to_string(),
                        data: None,
                    }),
                    id: Value::Null,
                }
            }
        };

        let response_json = serde_json::to_string(&response)?;
        info!("Sending response: {response_json}");

        writer.write_all(response_json.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    std::env::set_var("RUST_LOG", "debug");
    env_logger::init();

    let port = std::env::var("NEOVIM_MANAGER_PORT")
        .unwrap_or_else(|_| DEFAULT_PORT.to_string())
        .parse::<u16>()
        .unwrap_or(DEFAULT_PORT);

    let addr = format!("{DEFAULT_BIND_ADDR}:{port}");
    let listener = TcpListener::bind(&addr).await?;
    info!("Neovim Instance Manager listening on {addr}");

    let manager = Arc::new(InstanceManager::new());

    // 定期的なヘルスチェックタスクを開始
    let health_check_manager = Arc::clone(&manager);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(5));
        loop {
            interval.tick().await;
            if let Err(e) = health_check_manager.health_check_all().await {
                error!("Health check failed: {e}");
            }
        }
    });

    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                info!("New client connected from: {addr}");
                let manager_clone = Arc::clone(&manager);
                tokio::spawn(async move {
                    if let Err(e) = handle_client(stream, manager_clone).await {
                        error!("Error handling client: {e}");
                    }
                    info!("Client {addr} disconnected");
                });
            }
            Err(e) => {
                error!("Failed to accept connection: {e}");
            }
        }
    }
}
