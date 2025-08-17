use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use neovim_manager::{
    JsonRpcRequest, JsonRpcResponse, QueryInstanceParams, RegisterInstanceParams,
    UnregisterInstanceParams, DEFAULT_BIND_ADDR, DEFAULT_PORT,
};
use serde_json::{json, Value};
use std::process::Command;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::time::sleep;
use uuid::Uuid;

#[derive(Parser)]
#[command(name = "neovim-instance-manager-control")]
#[command(about = "Control client for neovim-instance-manager")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Query {
        identifier: String,
    },
    List,
    Register {
        identifier: String,
        server_address: String,
    },
    Unregister {
        identifier: String,
    },
    Shutdown,
}

struct ManagerClient {
    addr: String,
}

impl ManagerClient {
    fn new() -> Self {
        let port = std::env::var("NEOVIM_MANAGER_PORT")
            .unwrap_or_else(|_| DEFAULT_PORT.to_string())
            .parse::<u16>()
            .unwrap_or(DEFAULT_PORT);

        Self {
            addr: format!("{}:{}", DEFAULT_BIND_ADDR, port),
        }
    }

    async fn ensure_manager_running(&self) -> Result<()> {
        // まず接続を試行
        if TcpStream::connect(&self.addr).await.is_ok() {
            return Ok(());
        }

        // マネージャーを起動
        self.start_manager()?;

        // 起動を待つ（最大5秒）
        for i in 0..10 {
            sleep(Duration::from_millis(500)).await;
            if TcpStream::connect(&self.addr).await.is_ok() {
                return Ok(());
            }
            if i == 0 && std::env::var("NEOVIM_MANAGER_DEBUG").is_ok() {
                eprintln!("Starting manager, waiting for startup...");
            }
        }

        Err(anyhow!("Manager not responding after startup"))
    }

    fn start_manager(&self) -> Result<()> {
        use std::process::Stdio;

        // まず現在の実行可能ファイルのパスから推測
        let current_exe = std::env::current_exe()?;
        let manager_path = current_exe
            .parent()
            .ok_or_else(|| anyhow!("Cannot determine executable directory"))?
            .join("neovim-instance-manager");

        Command::new(&manager_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(())
    }

    async fn send_request(&self, method: &str, params: Value) -> Result<JsonRpcResponse> {
        self.ensure_manager_running().await?;

        let debug = std::env::var("NEOVIM_MANAGER_DEBUG").is_ok();

        if debug {
            eprintln!("Connecting to manager at {}", self.addr);
        }
        let mut stream = TcpStream::connect(&self.addr).await?;

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params,
            id: json!(Uuid::new_v4().to_string()),
        };

        let request_json = serde_json::to_string(&request)?;
        if debug {
            eprintln!("Sending request: {}", request_json);
        }

        stream.write_all(request_json.as_bytes()).await?;
        stream.write_all(b"\n").await?;
        stream.flush().await?;

        let (reader, _) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let mut line = String::new();

        if debug {
            eprintln!("Waiting for response...");
        }
        let bytes_read = reader.read_line(&mut line).await?;
        if debug {
            eprintln!("Read {} bytes: '{}'", bytes_read, line.trim());
        }

        if bytes_read == 0 {
            return Err(anyhow!("Connection closed by manager"));
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Err(anyhow!("Empty response from manager"));
        }

        let response: JsonRpcResponse = serde_json::from_str(trimmed)
            .map_err(|e| anyhow!("Failed to parse response '{}': {}", trimmed, e))?;

        Ok(response)
    }

    async fn query_instance(&self, identifier: &str) -> Result<()> {
        let params = serde_json::to_value(QueryInstanceParams {
            identifier: identifier.to_string(),
        })?;

        let response = self.send_request("query_instance", params).await?;

        if let Some(error) = response.error {
            eprintln!("Error: {} (code: {})", error.message, error.code);
            std::process::exit(1);
        }

        if let Some(result) = response.result {
            println!("{}", serde_json::to_string(&result)?);
        } else {
            println!("null");
        }

        Ok(())
    }

    async fn list_instances(&self) -> Result<()> {
        let response = self.send_request("list_instances", json!({})).await?;

        if let Some(error) = response.error {
            eprintln!("Error: {} (code: {})", error.message, error.code);
            std::process::exit(1);
        }

        if let Some(result) = response.result {
            println!("{}", serde_json::to_string_pretty(&result)?);
        }

        Ok(())
    }

    async fn register_instance(&self, identifier: &str, server_address: &str) -> Result<()> {
        let params = serde_json::to_value(RegisterInstanceParams {
            identifier: identifier.to_string(),
            server_address: server_address.to_string(),
        })?;

        let response = self.send_request("register_instance", params).await?;

        if let Some(error) = response.error {
            eprintln!("Error: {} (code: {})", error.message, error.code);
            std::process::exit(1);
        }

        if let Some(result) = response.result {
            println!("Success: {}", result.as_str().unwrap_or("registered"));
        }

        Ok(())
    }

    async fn unregister_instance(&self, identifier: &str) -> Result<()> {
        let params = serde_json::to_value(UnregisterInstanceParams {
            identifier: identifier.to_string(),
        })?;

        let response = self.send_request("unregister_instance", params).await?;

        if let Some(error) = response.error {
            eprintln!("Error: {} (code: {})", error.message, error.code);
            std::process::exit(1);
        }

        if let Some(result) = response.result {
            println!("Success: {}", result.as_str().unwrap_or("unregistered"));
        }

        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        let response = self.send_request("shutdown", json!({})).await?;

        if let Some(error) = response.error {
            eprintln!("Error: {} (code: {})", error.message, error.code);
            std::process::exit(1);
        }

        println!("Manager shutdown requested");
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let client = ManagerClient::new();

    match cli.command {
        Commands::Query { identifier } => {
            client.query_instance(&identifier).await?;
        }
        Commands::List => {
            client.list_instances().await?;
        }
        Commands::Register {
            identifier,
            server_address,
        } => {
            client
                .register_instance(&identifier, &server_address)
                .await?;
        }
        Commands::Unregister { identifier } => {
            client.unregister_instance(&identifier).await?;
        }
        Commands::Shutdown => {
            client.shutdown().await?;
        }
    }

    Ok(())
}
