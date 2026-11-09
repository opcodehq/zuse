# CLI presentation only. Commands, flags, URLs, credentials and MCP output retain their wire spelling.

cli-about = Zuse ブラウザークライアントとローカルエージェントエンジンを起動

cli-usage = 使用方法

cli-options = オプション

cli-commands = コマンド

cli-help = ヘルプを表示

cli-language = 表示言語、または system

cli-listen = ブラウザーサーバーの待ち受けアドレス

cli-ipc_port = エージェントツールのループバック RPC ポート。0 は空きポートを選択

cli-data_dir = ローカルデータディレクトリ

cli-assets = ブラウザークライアントのアセットディレクトリ

cli-token = ブラウザー認証トークン

cli-public_origin = 公開 HTTPS オリジン

cli-tls_cert = 直接 HTTPS 用の PEM 証明書チェーン

cli-tls_key = 対応する PEM 秘密鍵

cli-relay_origin = 公開リレーオリジン

cli-relay_host_token_file = リレールーティング認証情報ファイル

cli-mcp = エージェントツールを stdio で提供し、ローカルエンジンに転送

cli-invalid_arguments = 引数が無効です。以下の使用方法を確認してください。

cli-unknown_argument = 不明なオプションまたはコマンドです。以下の使用方法を確認してください。

cli-missing_argument = 必須の引数がありません。以下の使用方法を確認してください。

cli-relay_requires_credentials = リレーのアクセスにはオリジンとホストトークンファイルが必要です

cli-https_requires_keys = 直接 HTTPS には --tls-cert と --tls-key の両方が必要です

cli-https_required = ループバック以外での待ち受けには --tls-cert/--tls-key、または HTTPS リバースプロキシ用の --public-origin が必要です

cli-create_data_failed = ローカルデータディレクトリを作成できませんでした

cli-resolve_data_failed = ローカルデータディレクトリを解決できませんでした

cli-bind_failed = エージェントの RPC ポートを使用できません。別の --ipc-port を選んでください

cli-missing_home = HOME または --data-dir が必要です

cli-listening = エージェントツールの待ち受け先: 127.0.0.1:{ $port }

cli-open = 開く { $url }
