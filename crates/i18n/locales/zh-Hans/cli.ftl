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
