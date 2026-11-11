# CLI presentation only. Commands, flags, URLs, credentials and MCP output retain their wire spelling.

cli-about = 启动 Zuse 浏览器客户端和本地智能体引擎

cli-usage = 用法

cli-options = 选项

cli-commands = 命令

cli-help = 显示帮助

cli-language = 界面语言，或 system

cli-listen = 浏览器服务器监听地址

cli-ipc_port = 智能体工具的环回 RPC 端口；0 表示选择可用端口

cli-data_dir = 本地数据目录

cli-assets = 浏览器客户端资源目录

cli-token = 浏览器身份验证令牌

cli-public_origin = 公开 HTTPS 源地址

cli-tls_cert = 直接 HTTPS 连接的 PEM 证书链

cli-tls_key = 对应的 PEM 私钥

cli-relay_origin = 公开中继源地址

cli-relay_host_token_file = 中继路由凭据文件

cli-mcp = 通过 stdio 提供智能体工具并转发到本地引擎

cli-invalid_arguments = 参数无效。请查看以下用法。

cli-unknown_argument = 未知选项或命令。请查看以下用法。

cli-missing_argument = 缺少必需参数。请查看以下用法。

cli-relay_requires_credentials = 中继访问需要源地址和主机令牌文件

cli-https_requires_keys = 直接 HTTPS 需要同时提供 --tls-cert 和 --tls-key

cli-https_required = 非环回监听需要 --tls-cert/--tls-key，或 HTTPS 反向代理的 --public-origin

cli-create_data_failed = 无法创建本地数据目录

cli-resolve_data_failed = 无法解析本地数据目录

cli-bind_failed = 无法绑定智能体 RPC 端口；请选择其他 --ipc-port

cli-missing_home = 需要 HOME 或 --data-dir

cli-listening = 智能体工具监听于 127.0.0.1:{ $port }

cli-open = 打开 { $url }

cli-account = 在服务器启动时登录并关联这台电脑

cli-pair = 为其他设备创建私密链接

cli-pair_label = 在“已连接设备”中显示的名称

cli-pair_app = 显示用于 Zuse 应用而非浏览器的二维码

cli-login = 登录你的 Zuse 账号

cli-logout = 退出你的 Zuse 账号

cli-link = 让这台电脑可通过你的 Zuse 账号访问

cli-link_name = 在你的账号中显示的电脑名称

cli-unlink = 停止通过你的账号提供这台电脑

cli-status = 显示账号和网络状态

cli-server_unavailable = 代理端口 { $port } 上没有运行 Zuse 服务器。请先启动 zuse-server，或指定 --ipc-port。

cli-default_device = 新设备

cli-browser_link = 浏览器：{ $url }

cli-app_link = Zuse 应用：{ $url }

cli-certificate_warning = 首次打开时，浏览器会提示这台电脑的私有证书。请仅在可信网络中继续。

cli-link_warning = 任何持有此链接的人都可以使用这台电脑。未使用的链接将在 10 分钟后失效。

cli-local_only = 此链接仅在这台电脑上有效。请使用 --lan 或 --account 启动 zuse-server 以连接其他设备。

cli-sign_in_prompt = 打开 { $url } 并确认代码 { $code }

cli-signed_in = 已登录为 { $email }

cli-signed_out = 已退出登录

cli-sign_in_failed = 登录未完成

cli-linked = 已可通过你的账号访问，名称为 { $name }，地址为 { $url }

cli-unlinked = 这台电脑已无法再通过你的账号访问

cli-status_signed_in = 账号：{ $email }

cli-status_signed_out = 账号：未登录

cli-status_linked = 电脑：{ $name }，地址为 { $url }

cli-status_unlinked = 电脑：未关联账号

cli-status_no_routes = 网络：仅限这台电脑

cli-assets_missing = 未找到浏览器文件。请指定 --assets 或设置 ZUSE_WEB_ASSETS，否则浏览器无法打开此服务器。

cli-invitation_failed = 无法创建连接链接
