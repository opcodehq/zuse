# CLI presentation only. Commands, flags, URLs, credentials and MCP output retain their wire spelling.

cli-about = 啟動 Zuse 瀏覽器用戶端及本機代理程式引擎

cli-usage = 用法

cli-options = 選項

cli-commands = 指令

cli-help = 顯示說明

cli-language = 介面語言，或 system

cli-listen = 瀏覽器伺服器監聽位址

cli-ipc_port = 代理程式工具的回送 RPC 連接埠；0 表示選擇可用連接埠

cli-data_dir = 本機資料目錄

cli-assets = 瀏覽器用戶端資源目錄

cli-token = 瀏覽器驗證權杖

cli-public_origin = 公開 HTTPS 來源位址

cli-tls_cert = 直接 HTTPS 連線的 PEM 憑證鏈

cli-tls_key = 對應的 PEM 私密金鑰

cli-relay_origin = 公開轉送來源位址

cli-relay_host_token_file = 轉送路由認證資料檔案

cli-mcp = 透過 stdio 提供代理程式工具並轉送至本機引擎

cli-invalid_arguments = 引數無效。請查看以下用法。

cli-unknown_argument = 未知選項或指令。請查看以下用法。

cli-missing_argument = 缺少必要引數。請查看以下用法。

cli-relay_requires_credentials = 轉送存取需要來源位址及主機權杖檔案

cli-https_requires_keys = 直接 HTTPS 需要同時提供 --tls-cert 及 --tls-key

cli-https_required = 非回送監聽需要 --tls-cert/--tls-key，或 HTTPS 反向 Proxy 的 --public-origin

cli-create_data_failed = 無法建立本機資料目錄

cli-resolve_data_failed = 無法解析本機資料目錄

cli-bind_failed = 無法繫結代理程式 RPC 連接埠；請選擇其他 --ipc-port

cli-missing_home = 需要 HOME 或 --data-dir

cli-listening = 代理程式工具監聽於 127.0.0.1:{ $port }

cli-open = 開啟 { $url }
