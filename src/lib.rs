use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const DEFAULT_PORT: u16 = 57394;
pub const DEFAULT_BIND_ADDR: &str = "127.0.0.1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceInfo {
    pub identifier: String,
    pub server_address: String,
    pub registered_at: chrono::DateTime<chrono::Utc>,
    pub last_ping: chrono::DateTime<chrono::Utc>,
    pub health_status: HealthStatus,
    pub last_health_check: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HealthStatus {
    Unknown,
    Healthy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub method: String,
    pub params: serde_json::Value,
    pub id: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
    pub id: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

pub mod errors {
    pub const INSTANCE_ALREADY_EXISTS: i32 = -32001;
    pub const INSTANCE_NOT_FOUND: i32 = -32002;
    pub const HEALTH_CHECK_FAILED: i32 = -32003;
    pub const INTERNAL_ERROR: i32 = -32000;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryInstanceParams {
    pub identifier: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterInstanceParams {
    pub identifier: String,
    pub server_address: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnregisterInstanceParams {
    pub identifier: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceResult {
    pub identifier: String,
    pub server_address: String,
    pub health_status: HealthStatus,
    pub last_health_check: chrono::DateTime<chrono::Utc>,
}

pub type InstanceStorage = HashMap<String, InstanceInfo>;

pub mod utils {
    use std::process::Command;
    use anyhow::Result;

    pub fn check_nvim_instance(server_address: &str) -> Result<bool> {
        let output = Command::new("nvim")
            .args([
                "--server",
                server_address,
                "--remote-expr",
                "1",
            ])
            .output()?;
        
        Ok(output.status.success())
    }

    pub fn focus_nvim_instance(server_address: &str) -> Result<()> {
        Command::new("nvim")
            .args([
                "--server",
                server_address,
                "--remote-expr",
                "execute('NeovideFocus')",
            ])
            .output()?;
        
        Ok(())
    }

    pub fn quit_nvim_instance(server_address: &str) -> Result<bool> {
        let output = Command::new("nvim")
            .args([
                "--server",
                server_address,
                "--remote-expr",
                "execute('quit')",
            ])
            .output()?;
        
        Ok(output.status.success())
    }

    pub fn quit_nvim_instance_with_retry(server_address: &str, max_retries: u32) -> Result<()> {
        for attempt in 1..=max_retries {
            match quit_nvim_instance(server_address) {
                Ok(true) => {
                    eprintln!("Successfully sent quit to {}", server_address);
                    return Ok(());
                }
                Ok(false) => {
                    eprintln!("Quit command failed for {} (attempt {}/{})", server_address, attempt, max_retries);
                }
                Err(e) => {
                    eprintln!("Error sending quit to {} (attempt {}/{}): {}", server_address, attempt, max_retries, e);
                }
            }
            
            if attempt < max_retries {
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        }
        
        Err(anyhow::anyhow!("Failed to quit Neovim instance after {} attempts", max_retries))
    }

    pub fn get_random_port() -> Result<u16> {
        use std::net::TcpListener;
        
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let addr = listener.local_addr()?;
        drop(listener);
        
        Ok(addr.port())
    }

    pub fn is_wsl() -> bool {
        std::env::var("WSL_DISTRO_NAME").is_ok() ||
        std::fs::read_to_string("/proc/version")
            .map(|content| content.contains("Microsoft"))
            .unwrap_or(false)
    }

    pub fn get_neovide_command() -> &'static str {
        if is_wsl() || cfg!(windows) {
            "neovide.exe"
        } else {
            "neovide"
        }
    }
}