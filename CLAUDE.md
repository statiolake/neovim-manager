# Neovim Instance Manager 仕様書

## 概要

VS Codeライクな「同じプロジェクトなら既存ウィンドウをアクティブ化、新しいプロジェクトなら新しいウィンドウ」挙動をNeovim GUIで実現するためのシステム。

## システム構成

3つのコンポーネントで構成されます：

1. **neovim-instance-manager**: Neovimインスタンスを管理するデーモン
2. **neovim-instance-manager-control**: managerへの低レベルアクセスを提供するクライアント
3. **neovim-launcher**: ユーザー向けの統合インターフェース

launcherはcontrolを使用してmanagerにコマンドを送り、managerは実際のNeovim + Neovideインスタンスを管理します。

## 1. neovim-instance-manager (デーモン)

### 1.1 基本仕様

- **役割**: Neovimインスタンスの一元管理
- **動作形態**: バックグラウンドデーモン
- **通信方式**: TCP上のJSON-RPC
- **ポート番号**: `57394` (固定、衝突回避のため高位ポート使用)
- **バインドアドレス**: `127.0.0.1` (セキュリティのためlocalhostのみ)

### 1.2 管理データ構造

```json
{
  "instances": {
    "identifier_string": {
      "identifier": "string",
      "server_address": "ip:port",
      "registered_at": "timestamp",
      "last_ping": "timestamp"
    }
  }
}
```

### 1.3 JSON-RPC API

すべてのメソッドは JSON-RPC 2.0 仕様に準拠します。

#### 1.3.1 インスタンスクエリ

```json
// Request
{
  "jsonrpc": "2.0",
  "method": "query_instance",
  "params": {
    "identifier": "string"
  },
  "id": 1
}

// Success Response
{
  "jsonrpc": "2.0",
  "result": {
    "identifier": "string",
    "server_address": "ip:port"
  },
  "id": 1
}

// Not Found Response
{
  "jsonrpc": "2.0",
  "result": null,
  "id": 1
}
```

#### 1.3.2 インスタンス一覧

```json
// Request
{
  "jsonrpc": "2.0",
  "method": "list_instances",
  "params": {},
  "id": 2
}

// Response
{
  "jsonrpc": "2.0",
  "result": [
    {
      "identifier": "string",
      "server_address": "ip:port"
    }
  ],
  "id": 2
}
```

#### 1.3.3 インスタンス登録

```json
// Request
{
  "jsonrpc": "2.0",
  "method": "register_instance",
  "params": {
    "identifier": "string",
    "server_address": "ip:port"
  },
  "id": 3
}

// Success Response
{
  "jsonrpc": "2.0",
  "result": "registered",
  "id": 3
}

// Error Response (already exists)
{
  "jsonrpc": "2.0",
  "error": {
    "code": -32001,
    "message": "Instance already exists",
    "data": {
      "identifier": "string"
    }
  },
  "id": 3
}
```

#### 1.3.4 インスタンス削除

```json
// Request
{
  "jsonrpc": "2.0",
  "method": "unregister_instance",
  "params": {
    "identifier": "string"
  },
  "id": 4
}

// Success Response
{
  "jsonrpc": "2.0",
  "result": "unregistered",
  "id": 4
}

// Error Response (not found)
{
  "jsonrpc": "2.0",
  "error": {
    "code": -32002,
    "message": "Instance not found",
    "data": {
      "identifier": "string"
    }
  },
  "id": 4
}
```

#### 1.3.5 マネージャー終了

```json
// Request
{
  "jsonrpc": "2.0",
  "method": "shutdown",
  "params": {},
  "id": 5
}

// Response
{
  "jsonrpc": "2.0",
  "result": "shutting_down",
  "id": 5
}
```

### 1.4 動作仕様

#### 1.4.1 起動時動作

1. TCP ポート `57394` でリスンを開始
2. プロセスをデーモン化
3. 制御を呼び出し元に返す

#### 1.4.2 健全性チェック

- 各API呼び出し前に登録済みインスタンスへの疎通確認を実行
- 疎通方法: `nvim --server <server_address> --remote-expr "1"`
- 一度でも疎通した後で疎通不可になった場合、そのインスタンスを自動削除

#### 1.4.3 エラーコード定義

- `-32001`: インスタンス重複エラー
- `-32002`: インスタンス未発見エラー
- `-32003`: 疎通失敗エラー
- `-32000`: 内部エラー

## 2. neovim-instance-manager-control (低レベルクライアント)

### 2.1 基本仕様

- **役割**: manager への低レベルアクセス提供
- **実装形態**: ライブラリまたはCLIツール
- **自動起動**: manager が未起動の場合、透過的に起動

### 2.2 コマンドライン仕様

```bash
# インスタンスクエリ
neovim-instance-manager-control query <identifier>

# インスタンス一覧
neovim-instance-manager-control list

# インスタンス登録
neovim-instance-manager-control register <identifier> <server_address>

# インスタンス削除
neovim-instance-manager-control unregister <identifier>

# マネージャー終了
neovim-instance-manager-control shutdown
```

### 2.3 動作仕様

#### 2.3.1 自動起動ロジック

1. TCP ポート `57394` への接続を試行
2. 接続失敗時:
   - `neovim-instance-manager` を起動
   - 最大5秒間、1秒間隔で接続リトライ
   - 5秒経過後も接続できない場合はエラー終了

#### 2.3.2 タイムアウト設定

- 接続タイムアウト: 3秒
- 応答タイムアウト: 10秒

## 3. neovim-launcher (高レベルクライアント)

### 3.1 基本仕様

- **役割**: ユーザー向けの統合インターフェース
- **動作**: non-forkなNeovim GUIのように振る舞う
- **依存**: neovim-instance-manager-control を使用

### 3.2 コマンドライン仕様

```bash
# ローカル使用
neovim-launcher [file_or_directory]

# リモート使用
neovim-launcher --remote <server_address> --identifier <identifier>

# オプション
  --remote              リモートモードで実行
  --identifier STRING   リモート時のidentifier (必須)
  --help               ヘルプ表示
```

### 3.3 動作仕様

#### 3.3.1 Identifier生成ルール

**ローカルモード:**

```bash
# ディレクトリが指定された場合
identifier = realpath(directory)

# ファイルが指定された場合
identifier = realpath(dirname(file))

# 何も指定されない場合
identifier = realpath(getcwd())
```

**リモートモード:**

```bash
# --identifier で明示的に指定
identifier = user_provided_identifier
```

#### 3.3.2 実行フローチャート

```
Start
  │
  ├─ Parse arguments
  │
  ├─ Generate identifier
  │
  ├─ Query manager for existing instance
  │
  ├─ Instance exists?
  │    ├─ Yes → Focus existing instance → Monitor until exit
  │    └─ No  → Launch new instance → Register → Monitor until exit
  │
  └─ Exit with appropriate code
```

#### 3.3.3 新規インスタンス起動

**ランダムポート選定方法:**

- port 0でTCP Listenerを作成
- OSが自動割り当てしたポート番号を取得
- Listenerを即座にクローズ
- 取得したポート番号を使用

**ローカルモード:**

```bash
# 通常のLinux/macOS環境
neovide --server 127.0.0.1:$(allocated_port) -- --listen 127.0.0.1:$(allocated_port) $(target_directory)

# WSL環境 (Windows版Neovideを使用)
neovide.exe --server 127.0.0.1:$(allocated_port) -- --listen 127.0.0.1:$(allocated_port) $(target_directory)

# Windows環境
neovide.exe --server 127.0.0.1:$(allocated_port) -- --listen 127.0.0.1:$(allocated_port) $(target_directory)
```

**リモートモード:**

```bash
# 既存のリモートNeovimインスタンスに接続
# (事前にユーザーが `nvim --listen` で起動済みと仮定)

# 通常のLinux/macOS環境
neovide --server $(user_provided_server_address)

# WSL環境 (Windows版Neovideを使用)
neovide.exe --server $(user_provided_server_address)

# Windows環境
neovide.exe --server $(user_provided_server_address)
```

#### 3.3.4 WSL環境判定

WSL環境では自動的にWindows版Neovide (neovide.exe) を実行します：

- WSL環境の判定: 環境変数 `WSL_DISTRO_NAME` の存在または `/proc/version` に "Microsoft" が含まれる
- その他の処理 (identifier生成、管理ロジック等) は通常のLinux環境と同じ

#### 3.3.5 フォーカス実行

```bash
# 既存インスタンスにフォーカス要求
nvim --server <server_address> --remote-expr "execute('NeovideFocus')"
```

#### 3.3.6 監視ループ

- 500ms間隔で manager にインスタンス存在確認
- インスタンスが削除された場合 (= プロセス終了) 、launcher も終了
- 終了コード: 常に 0

### 3.4 エラーハンドリング

#### 3.4.1 ローカルモード

- 指定されたパスが存在しない → エラー終了 (code: 1)
- GUI起動失敗 → エラー終了 (code: 2)
- manager との通信エラー → エラー終了 (code: 3)

#### 3.4.2 リモートモード

- server_address への接続失敗 → エラー終了 (code: 4)
- identifier が未指定 → エラー終了 (code: 5)

## 4. 実装考慮事項

### 4.1 プラットフォーム対応

#### 4.1.1 パス正規化

- シンボリックリンクの解決 (`realpath` 使用)
- 大文字小文字の統一 (Windows: 無視、Unix: 保持)

### 4.2 セキュリティ

- TCP通信はlocalhostのみ
- 認証機能は実装しない (ローカル環境前提)
- プロセス権限は実行ユーザーと同等

### 4.3 ログ出力

#### 4.3.1 manager

- 標準出力: なし (デーモン化後)
- ログファイル: `~/.cache/neovim-instance-manager/manager.log`
- ログレベル: INFO, WARN, ERROR

#### 4.3.2 launcher/control

- 標準エラー出力にエラーメッセージ
- 詳細ログは `~/.cache/neovim-instance-manager/client.log`

### 4.4 設定ファイル

基本的に設定ファイルは使用せず、すべてコマンドライン引数で制御。
ただし、以下の環境変数をサポート:

```bash
export NEOVIM_MANAGER_PORT=57394        # デフォルトポート番号
export NEOVIM_MANAGER_TIMEOUT=10        # タイムアウト秒数
```

## 4. 実装優先度

### Phase 1 (MVP)

- neovim-instance-manager 基本機能
- neovim-instance-manager-control 基本機能
- neovim-launcher ローカル・リモートモード
- WSL環境でのWindows版Neovide自動実行
