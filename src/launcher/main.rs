use anyhow::{anyhow, Result};
use clap::Parser;
use log::{error, info, warn};
use neovim_manager::{utils, HealthStatus, InstanceResult};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;
use tokio::signal;
use tokio::sync::Mutex;
use tokio::time::sleep;

#[derive(Parser)]
#[command(name = "neovim-launcher")]
#[command(about = "High-level Neovim launcher with instance management")]
struct Cli {
    #[arg(help = "File or directory to open")]
    target: Option<PathBuf>,
    
    #[arg(long, help = "Remote mode")]
    remote: bool,
    
    #[arg(long, help = "Remote identifier (required for remote mode)")]
    identifier: Option<String>,
    
    #[arg(long, help = "Remote server address (required for remote mode)")]
    server: Option<String>,
}

struct LauncherClient {
    control_binary: String,
}

impl LauncherClient {
    fn new() -> Result<Self> {
        let current_exe = std::env::current_exe()?;
        let control_path = current_exe
            .parent()
            .ok_or_else(|| anyhow!("Cannot determine executable directory"))?
            .join("neovim-instance-manager-control");
        
        Ok(Self {
            control_binary: control_path.to_string_lossy().to_string(),
        })
    }

    async fn query_instance(&self, identifier: &str) -> Result<Option<InstanceResult>> {
        let output = Command::new(&self.control_binary)
            .args(["query", identifier])
            .output()?;

        if !output.status.success() {
            return Ok(None);
        }

        let stdout = String::from_utf8(output.stdout)?;
        let trimmed = stdout.trim();
        
        let result: Option<InstanceResult> = serde_json::from_str(trimmed)
            .map_err(|e| anyhow!("Failed to parse JSON from stdout '{}': {}", trimmed, e))?;
        
        Ok(result)
    }

    async fn register_instance(&self, identifier: &str, server_address: &str) -> Result<()> {
        let output = Command::new(&self.control_binary)
            .args(["register", identifier, server_address])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8(output.stderr)?;
            return Err(anyhow!("Failed to register instance: {}", stderr));
        }

        Ok(())
    }

    async fn monitor_instance(&self, identifier: &str) -> Result<()> {
        info!("Monitoring instance: {}", identifier);
        
        loop {
            match self.query_instance(identifier).await {
                Ok(Some(_)) => {
                    sleep(Duration::from_millis(500)).await;
                }
                Ok(None) => {
                    info!("Instance {} no longer exists, exiting", identifier);
                    break;
                }
                Err(e) => {
                    warn!("Error monitoring instance {}: {}", identifier, e);
                    sleep(Duration::from_millis(500)).await;
                }
            }
        }

        Ok(())
    }

    async fn monitor_instance_with_exit_code(&self, identifier: &str, nvim_process: Child) -> Result<i32> {
        info!("Monitoring instance: {}", identifier);
        
        let mut nvim_process = nvim_process;
        
        loop {
            match self.query_instance(identifier).await {
                Ok(Some(_)) => {
                    sleep(Duration::from_millis(500)).await;
                }
                Ok(None) => {
                    info!("Instance {} no longer exists, checking exit code", identifier);
                    
                    // Neovimプロセスの終了を待機して終了コードを取得
                    match nvim_process.wait() {
                        Ok(status) => {
                            let exit_code = status.code().unwrap_or(-1);
                            info!("Neovim process exited with code: {}", exit_code);
                            return Ok(exit_code);
                        }
                        Err(e) => {
                            error!("Failed to wait for Neovim process: {}", e);
                            return Ok(-1);
                        }
                    }
                }
                Err(e) => {
                    warn!("Error monitoring instance {}: {}", identifier, e);
                    sleep(Duration::from_millis(500)).await;
                }
            }
        }
    }
}

fn generate_identifier(target: Option<&PathBuf>) -> Result<String> {
    let path = match target {
        Some(path) => {
            if path.is_dir() {
                path.clone()
            } else {
                path.parent()
                    .ok_or_else(|| anyhow!("Cannot determine parent directory"))?
                    .to_path_buf()
            }
        }
        None => std::env::current_dir()?,
    };

    let canonical = path.canonicalize()?;
    Ok(canonical.to_string_lossy().to_string())
}

fn launch_neovim_server(_identifier: &str, target_dir: Option<&PathBuf>, target_file: Option<&PathBuf>, server_address: &str) -> Result<Child> {
    let dir_arg = target_dir
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| ".".to_string());

    let mut args = vec![
        "--listen".to_string(), 
        server_address.to_string(),
        "--headless".to_string(),
    ];

    // ファイルが指定されている場合はそれを引数として追加
    if let Some(file_path) = target_file {
        args.push(file_path.to_string_lossy().to_string());
    } else {
        args.push(dir_arg);
    }

    let args_str: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

    eprintln!("Executing: nvim {}", args.join(" "));
    info!("Launching Neovim server: {}", server_address);

    let mut nvim_cmd = Command::new("nvim");
    nvim_cmd.args(&args_str);
    
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        nvim_cmd.creation_flags(0x08000000);
    }
    
    #[cfg(not(windows))]
    {
        nvim_cmd.stdin(Stdio::null())
               .stdout(Stdio::null())
               .stderr(Stdio::null());
    }
    
    let nvim_child = nvim_cmd.spawn()?;
    eprintln!("Nvim server spawned with PID: {:?}", nvim_child.id());
    std::thread::sleep(Duration::from_millis(1000));
    
    Ok(nvim_child)
}

fn launch_neovide_client(server_address: &str) -> Result<()> {
    let neovide_cmd = utils::get_neovide_command();
    let args = ["--server", server_address];

    eprintln!("Executing: {} {}", neovide_cmd, args.join(" "));
    info!("Launching Neovide client for server: {}", server_address);

    let mut cmd = Command::new(neovide_cmd);
    cmd.args(args);

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    
    #[cfg(not(windows))]
    {
        cmd.stdin(Stdio::null())
           .stdout(Stdio::null())
           .stderr(Stdio::null());
    }

    let _child = cmd.spawn()?;
    eprintln!("Neovide client spawned successfully");
    std::thread::sleep(Duration::from_millis(500));
    
    Ok(())
}

async fn focus_existing_instance(server_address: &str, target_file: Option<&PathBuf>) -> Result<()> {
    info!("Focusing existing instance: {}", server_address);
    
    // CLAUDE.mdに従ってNeovideFocusコマンドを実行
    utils::focus_nvim_instance(server_address)?;
    
    // ファイルが指定されている場合は、そのファイルをリモートで開く
    if let Some(file_path) = target_file {
        let file_str = file_path.to_string_lossy();
        info!("Opening file in existing instance: {}", file_str);
        utils::open_file_in_nvim_instance(server_address, &file_str)?;
    }
    
    Ok(())
}

#[derive(Debug, Clone)]
struct CleanupInfo {
    server_address: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    
    let cli = Cli::parse();
    let client = LauncherClient::new()?;
    
    // クリーンアップ情報を管理
    let cleanup_info = Arc::new(Mutex::new(CleanupInfo {
        server_address: None,
    }));

    // ローカルモードでのファイル/ディレクトリ処理
    let (target_dir, target_file) = if cli.remote {
        (cli.target.clone(), None)
    } else {
        match &cli.target {
            Some(path) if path.is_file() => {
                // ファイルが指定された場合：ディレクトリは.(カレント)、ファイルを記録
                let file_path = path.canonicalize()?;
                (None, Some(file_path)) // target_dirはNoneにして常に"."を使用
            }
            _ => {
                // ディレクトリまたは未指定の場合
                (cli.target.clone(), None)
            }
        }
    };

    let identifier = if cli.remote {
        cli.identifier
            .ok_or_else(|| anyhow!("--identifier is required in remote mode"))?
    } else {
        // ファイル指定の場合でも現在のディレクトリをidentifierに使用
        let identifier_target = if target_file.is_some() {
            None // ファイル指定時は現在のディレクトリを使用
        } else {
            target_dir.as_ref()
        };
        generate_identifier(identifier_target)?
    };

    info!("Using identifier: {}", identifier);

    // Ctrl+C ハンドラーを設定
    let cleanup_info_clone = Arc::clone(&cleanup_info);
    tokio::spawn(async move {
        if let Err(e) = signal::ctrl_c().await {
            error!("Failed to listen for ctrl-c: {}", e);
            return;
        }
        
        info!("Received Ctrl+C, performing cleanup...");
        let cleanup = cleanup_info_clone.lock().await;
        
        if let Some(server_address) = &cleanup.server_address {
            eprintln!("Cleaning up unused Neovim server: {}", server_address);
            if let Err(e) = utils::quit_nvim_instance_with_retry(server_address, 3) {
                eprintln!("Failed to cleanup server: {}", e);
            }
        }
        
        std::process::exit(0);
    });

    if cli.remote {
        let server_address = cli.server
            .ok_or_else(|| anyhow!("--server is required in remote mode"))?;

        // リモートモードでは既存インスタンスをチェック
        match client.query_instance(&identifier).await? {
            Some(instance) => {
                info!("Found existing remote instance");
                
                // 既存インスタンスが見つかった場合、新規サーバーをクリーンアップ対象に設定
                {
                    let mut cleanup = cleanup_info.lock().await;
                    cleanup.server_address = Some(server_address.clone());
                }
                
                // 既存インスタンスにフォーカス（CLAUDE.md仕様）
                focus_existing_instance(&instance.server_address, None).await?;
                
                // 監視終了後、新規サーバーをクリーンアップ
                let result = client.monitor_instance(&identifier).await;
                
                eprintln!("Cleaning up unused Neovim server: {}", server_address);
                if let Err(e) = utils::quit_nvim_instance_with_retry(&server_address, 3) {
                    eprintln!("Failed to cleanup server: {}", e);
                }
                
                result?;
            }
            None => {
                info!("Registering new remote instance");
                client.register_instance(&identifier, &server_address).await?;
                
                // 新規リモートインスタンスにNeovideクライアントで接続
                launch_neovide_client(&server_address)?;
                
                client.monitor_instance(&identifier).await?;
            }
        }
    } else {
        // ローカルモード
        match client.query_instance(&identifier).await? {
            Some(instance) => {
                info!("Found existing local instance");
                focus_existing_instance(&instance.server_address, target_file.as_ref()).await?;
                client.monitor_instance(&identifier).await?;
            }
            None => {
                // 終了コード2の場合は再起動ループ
                loop {
                    info!("Creating new local instance");
                    let port = utils::get_random_port()?;
                    let server_address = format!("127.0.0.1:{}", port);
                    
                    // Neovimサーバーを起動
                    let nvim_process = launch_neovim_server(&identifier, target_dir.as_ref(), target_file.as_ref(), &server_address)?;
                    
                    // Neovimインスタンスが起動するまで待機
                    info!("Waiting for Neovim instance to start...");
                    let mut attempts = 0;
                    let max_attempts = 30; // 15秒間待機
                    
                    loop {
                        if utils::check_nvim_instance(&server_address).unwrap_or(false) {
                            info!("Neovim instance is ready");
                            break;
                        }
                        
                        attempts += 1;
                        if attempts >= max_attempts {
                            error!("Neovim instance failed to start within 15 seconds");
                            std::process::exit(3);
                        }
                        
                        sleep(Duration::from_millis(500)).await;
                    }
                    
                    // インスタンスを登録
                    match client.register_instance(&identifier, &server_address).await {
                        Ok(()) => {
                            info!("Instance registered successfully");
                            
                            // 登録直後の確認（即座に登録されているはず）
                            match client.query_instance(&identifier).await? {
                                Some(instance) => {
                                    info!("Instance registration confirmed");
                                    
                                    // ヘルスステータスがHealthyになるまで待機
                                    if !matches!(instance.health_status, HealthStatus::Healthy) {
                                        info!("Waiting for instance to become healthy...");
                                        let mut attempts = 0;
                                        let max_attempts = 60; // 30秒間待機（5秒間隔のヘルスチェック）
                                        
                                        loop {
                                            sleep(Duration::from_millis(500)).await;
                                            
                                            match client.query_instance(&identifier).await? {
                                                Some(updated_instance) => {
                                                    if matches!(updated_instance.health_status, HealthStatus::Healthy) {
                                                        info!("Instance is now healthy");
                                                        break;
                                                    }
                                                }
                                                None => {
                                                    error!("Instance disappeared during health check wait");
                                                    std::process::exit(5);
                                                }
                                            }
                                            
                                            attempts += 1;
                                            if attempts >= max_attempts {
                                                error!("Instance did not become healthy within 30 seconds");
                                                std::process::exit(6);
                                            }
                                        }
                                    } else {
                                        info!("Instance is already healthy");
                                    }
                                    
                                    // Neovide クライアントを起動
                                    launch_neovide_client(&server_address)?;
                                }
                                None => {
                                    error!("Instance not found immediately after registration - this should not happen");
                                    std::process::exit(4);
                                }
                            }
                            
                            // 監視して終了コードを取得
                            let exit_code = client.monitor_instance_with_exit_code(&identifier, nvim_process).await?;
                            
                            if exit_code == 2 {
                                info!("Neovim exited with code 2, restarting...");
                                continue; // 再起動ループを継続
                            } else {
                                info!("Neovim exited with code {}, ending", exit_code);
                                break; // ループを抜けて終了
                            }
                        }
                        Err(e) => {
                            error!("Failed to register instance: {}", e);
                            std::process::exit(2);
                        }
                    }
                }
            }
        }
    }

    Ok(())
}