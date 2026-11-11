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

cli-account = 在伺服器啟動時登入並連結這台電腦

cli-pair = 為其他裝置建立私密連結

cli-pair_label = 在「已連線裝置」中顯示的名稱

cli-pair_app = 顯示用於 Zuse 應用程式而非瀏覽器的 QR 碼

cli-login = 登入你的 Zuse 帳號

cli-logout = 登出你的 Zuse 帳號

cli-link = 讓這台電腦可透過你的 Zuse 帳號存取

cli-link_name = 在你的帳號中顯示的電腦名稱

cli-unlink = 停止透過你的帳號提供這台電腦

cli-status = 顯示帳號和網路狀態

cli-server_unavailable = 代理連接埠 { $port } 上沒有執行 Zuse 伺服器。請先啟動 zuse serve，或指定 --ipc-port。

cli-default_device = 新裝置

cli-browser_link = 瀏覽器：{ $url }

cli-app_link = Zuse 應用程式：{ $url }

cli-certificate_warning = 首次開啟時，瀏覽器會提示這台電腦的私有憑證。請僅在可信任的網路中繼續。

cli-link_warning = 任何持有此連結的人都可以使用這台電腦。未使用的連結將在 10 分鐘後失效。

cli-local_only = 此連結僅在這台電腦上有效。請使用 --lan 或 --account 啟動 zuse serve 以連線其他裝置。

cli-sign_in_prompt = 開啟 { $url } 並確認代碼 { $code }

cli-signed_in = 已登入為 { $email }

cli-signed_out = 已登出

cli-sign_in_failed = 登入未完成

cli-linked = 已可透過你的帳號存取，名稱為 { $name }，位址為 { $url }

cli-unlinked = 這台電腦已無法再透過你的帳號存取

cli-status_signed_in = 帳號：{ $email }

cli-status_signed_out = 帳號：未登入

cli-status_linked = 電腦：{ $name }，位址為 { $url }

cli-status_unlinked = 電腦：未連結帳號

cli-status_no_routes = 網路：僅限這台電腦

cli-assets_missing = 找不到瀏覽器檔案。請指定 --assets 或設定 ZUSE_WEB_ASSETS，否則瀏覽器無法開啟此伺服器。

cli-invitation_failed = 無法建立連線連結

cli-zuse_about = 適用於程式設計代理程式的多裝置控制器

cli-arguments = 引數

cli-version = 顯示版本

cli-engine_port = 引擎的回送 RPC 連接埠，預設為 ZUSE_IPC_PORT 或 27654；用於 serve 時，0 表示選擇可用連接埠

cli-lan = 允許此網路和 tailnet 中已配對的裝置直接連線，全程端對端加密

cli-lan_port = --lan 連線使用的連接埠

cli-tailscale_serve = 同時透過 Tailscale HTTPS 發布 --lan，供瀏覽器存取

cli-headless = 在沒有視窗的情況下執行引擎

cli-overview_status = 顯示工作區、引擎和遠端存取狀態

cli-session_login = 登入，並在下次啟動引擎時開啟工作區同步

cli-login_no_browser = 輸出登入連結，而不是在這台電腦上開啟瀏覽器

cli-session_logout = 刪除已儲存的工作階段；下次啟動引擎時僅限本機

cli-sync = 顯示執行中引擎的即時同步狀態

cli-daemon = 將 Zuse 作為背景服務執行

cli-daemon_install = 安裝、啟用並啟動服務

cli-daemon_serve = 使用後面的引數執行 zuse serve，而不是 zuse headless

cli-daemon_serve_args = zuse serve 的引數

cli-daemon_uninstall = 停止並移除服務

cli-daemon_start = 啟動已安裝的服務

cli-daemon_stop = 停止服務

cli-daemon_restart = 重新啟動服務

cli-daemon_status = 顯示服務狀態

cli-update = 檢查新版本並安裝

cli-update_check = 只報告是否有可用更新

cli-appshot = 在執行中的應用程式中擷取 Appshot

cli-open_url = 要開啟的 Zuse 連結

cli-paired_link_file = 包含配對連結的私密檔案，用於連線

cli-desktop_missing = 這裡沒有安裝 Zuse 桌面應用程式，因此無法開啟視窗。請改用下列指令。

cli-status_remote = 遠端存取
