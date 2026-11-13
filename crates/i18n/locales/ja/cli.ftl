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

cli-account = サーバーの起動時にサインインしてこのコンピューターをリンクする

cli-pair = 別のデバイス用のプライベートリンクを作成する

cli-pair_label = 接続済みデバイスに表示する名前

cli-pair_app = ブラウザーではなく Zuse アプリ用の QR コードを表示する

cli-login = Zuse アカウントにサインインする

cli-logout = Zuse アカウントからサインアウトする

cli-link = Zuse アカウント経由でこのコンピューターを利用できるようにする

cli-link_name = アカウントに表示するコンピューター名

cli-unlink = アカウント経由でのこのコンピューターの提供を停止する

cli-status = アカウントとネットワークの状態を表示する

cli-server_unavailable = エージェントポート { $port } で Zuse サーバーが実行されていません。先に zuse serve を起動するか、--ipc-port を指定してください。

cli-default_device = 新しいデバイス

cli-browser_link = ブラウザー: { $url }

cli-app_link = Zuse アプリ: { $url }

cli-certificate_warning = 初回はブラウザーがこのコンピューターのプライベート証明書について警告します。信頼できるネットワークでのみ続行してください。

cli-link_warning = このリンクを持つ人は誰でもこのコンピューターを使用できます。未使用のリンクは 10 分で期限切れになります。

cli-local_only = このリンクはこのコンピューターでのみ機能します。他のデバイスを接続するには --lan または --account を指定して zuse serve を起動してください。

cli-sign_in_prompt = { $url } を開いてコード { $code } を確認してください

cli-signed_in = { $email } としてサインインしました

cli-signed_out = サインアウトしました

cli-sign_in_failed = サインインが完了しませんでした

cli-linked = アカウント経由で { $name } として { $url } で利用できます

cli-unlinked = このコンピューターはアカウント経由で利用できなくなりました

cli-status_signed_in = アカウント: { $email }

cli-status_signed_out = アカウント: サインインしていません

cli-status_linked = コンピューター: { $name } ({ $url })

cli-status_unlinked = コンピューター: アカウントにリンクされていません

cli-status_no_routes = ネットワーク: このコンピューターのみ

cli-assets_missing = ブラウザー用ファイルが見つかりません。--assets を指定するか ZUSE_WEB_ASSETS を設定するまで、ブラウザーではこのサーバーを開けません。

cli-invitation_failed = 接続リンクを作成できませんでした

cli-zuse_about = コーディングエージェント向けのマルチデバイスコントローラー

cli-arguments = 引数

cli-version = バージョンを表示

cli-engine_port = エンジンのループバック RPC ポート。既定は ZUSE_IPC_PORT または 27654。serve では 0 で空きポートを選択

cli-lan = このネットワークと tailnet 上のペアリング済みデバイスから直接接続できるようにする。通信はエンドツーエンドで暗号化

cli-lan_port = --lan 接続のポート

cli-tailscale_serve = --lan をブラウザー向けに Tailscale HTTPS でも公開する

cli-headless = ウィンドウなしでエンジンを実行する

cli-overview_status = ワークスペース、エンジン、リモートアクセスの状態を表示する

cli-session_login = サインインし、次回のエンジン起動時にワークスペースの同期をオンにする

cli-login_no_browser = このコンピューターでブラウザーを開く代わりにサインインリンクを表示する

cli-session_logout = 保存されたセッションを削除する。次回のエンジン起動はローカルのみ

cli-sync = 実行中のエンジンの同期状態を表示する

cli-daemon = Zuse をバックグラウンドサービスとして実行する

cli-daemon_install = サービスをインストール、有効化して起動する

cli-daemon_serve = zuse headless の代わりに、続く引数で zuse serve を実行する

cli-daemon_serve_args = zuse serve の引数

cli-daemon_uninstall = サービスを停止して削除する

cli-daemon_start = インストール済みのサービスを起動する

cli-daemon_stop = サービスを停止する

cli-daemon_restart = サービスを再起動する

cli-daemon_status = サービスの状態を表示する

cli-update = 新しいリリースを確認してインストールする

cli-update_check = 更新があるかどうかだけを表示する

cli-appshot = 実行中のアプリで Appshot を撮る

cli-open_url = 開く Zuse リンク

cli-paired_link_file = 接続に使うペアリングリンクを含むプライベートファイル

cli-desktop_missing = ここには Zuse デスクトップアプリがインストールされていないため、ウィンドウを開けません。代わりに次のコマンドを使ってください。

cli-status_remote = リモートアクセス
