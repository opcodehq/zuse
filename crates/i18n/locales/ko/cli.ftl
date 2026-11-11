# CLI presentation only. Commands, flags, URLs, credentials and MCP output retain their wire spelling.

cli-about = Zuse 브라우저 클라이언트 및 로컬 에이전트 엔진 제공

cli-usage = 사용법

cli-options = 옵션

cli-commands = 명령

cli-help = 도움말 표시

cli-language = 인터페이스 언어 또는 system

cli-listen = 브라우저 서버 수신 주소

cli-ipc_port = 에이전트 도구의 루프백 RPC 포트. 0은 사용 가능한 포트 선택

cli-data_dir = 로컬 데이터 디렉터리

cli-assets = 브라우저 클라이언트 리소스 디렉터리

cli-token = 브라우저 인증 토큰

cli-public_origin = 공개 HTTPS 출처

cli-tls_cert = 직접 HTTPS 연결용 PEM 인증서 체인

cli-tls_key = 해당 PEM 개인 키

cli-relay_origin = 공개 릴레이 출처

cli-relay_host_token_file = 릴레이 라우팅 자격 증명 파일

cli-mcp = stdio로 에이전트 도구를 제공하고 로컬 엔진으로 전달

cli-invalid_arguments = 잘못된 인수입니다. 아래 사용법을 확인하세요.

cli-unknown_argument = 알 수 없는 옵션 또는 명령입니다. 아래 사용법을 확인하세요.

cli-missing_argument = 필수 인수가 없습니다. 아래 사용법을 확인하세요.

cli-relay_requires_credentials = 릴레이 액세스에는 출처 및 호스트 토큰 파일이 필요합니다

cli-https_requires_keys = 직접 HTTPS에는 --tls-cert 및 --tls-key가 모두 필요합니다

cli-https_required = 루프백 외 수신에는 --tls-cert/--tls-key 또는 HTTPS 리버스 프록시용 --public-origin이 필요합니다

cli-create_data_failed = 로컬 데이터 디렉터리를 만들 수 없습니다

cli-resolve_data_failed = 로컬 데이터 디렉터리를 확인할 수 없습니다

cli-bind_failed = 에이전트 RPC 포트를 바인딩할 수 없습니다. 다른 --ipc-port를 선택하세요

cli-missing_home = HOME 또는 --data-dir가 필요합니다

cli-listening = 에이전트 도구 수신 주소: 127.0.0.1:{ $port }

cli-open = 열기 { $url }

cli-account = 서버 시작 시 로그인하고 이 컴퓨터를 연결합니다

cli-pair = 다른 기기용 비공개 링크를 만듭니다

cli-pair_label = 연결된 기기에 표시할 이름

cli-pair_app = 브라우저 대신 Zuse 앱용 QR 코드를 표시합니다

cli-login = Zuse 계정에 로그인합니다

cli-logout = Zuse 계정에서 로그아웃합니다

cli-link = Zuse 계정을 통해 이 컴퓨터를 사용할 수 있게 합니다

cli-link_name = 계정에 표시할 컴퓨터 이름

cli-unlink = 계정을 통한 이 컴퓨터 제공을 중단합니다

cli-status = 계정 및 네트워크 상태를 표시합니다

cli-server_unavailable = 에이전트 포트 { $port }에서 실행 중인 Zuse 서버가 없습니다. 먼저 zuse serve를 시작하거나 --ipc-port를 지정하세요.

cli-default_device = 새 기기

cli-browser_link = 브라우저: { $url }

cli-app_link = Zuse 앱: { $url }

cli-certificate_warning = 처음에는 브라우저가 이 컴퓨터의 비공개 인증서에 대해 경고합니다. 신뢰할 수 있는 네트워크에서만 계속하세요.

cli-link_warning = 이 링크가 있는 사람은 누구나 이 컴퓨터를 사용할 수 있습니다. 사용하지 않은 링크는 10분 후 만료됩니다.

cli-local_only = 이 링크는 이 컴퓨터에서만 작동합니다. 다른 기기를 연결하려면 --lan 또는 --account로 zuse serve를 시작하세요.

cli-sign_in_prompt = { $url }을(를) 열고 코드 { $code }을(를) 확인하세요

cli-signed_in = { $email }(으)로 로그인했습니다

cli-signed_out = 로그아웃했습니다

cli-sign_in_failed = 로그인이 완료되지 않았습니다

cli-linked = 계정을 통해 { $name }(으)로 { $url }에서 사용할 수 있습니다

cli-unlinked = 이 컴퓨터는 더 이상 계정을 통해 사용할 수 없습니다

cli-status_signed_in = 계정: { $email }

cli-status_signed_out = 계정: 로그인하지 않음

cli-status_linked = 컴퓨터: { $name } ({ $url })

cli-status_unlinked = 컴퓨터: 계정에 연결되지 않음

cli-status_no_routes = 네트워크: 이 컴퓨터만

cli-assets_missing = 브라우저 파일을 찾을 수 없습니다. --assets를 지정하거나 ZUSE_WEB_ASSETS를 설정해야 브라우저에서 이 서버를 열 수 있습니다.

cli-invitation_failed = 연결 링크를 만들 수 없습니다

cli-zuse_about = 코딩 에이전트를 위한 다중 기기 컨트롤러

cli-arguments = 인수

cli-version = 버전 표시

cli-engine_port = 엔진의 루프백 RPC 포트. 기본값은 ZUSE_IPC_PORT 또는 27654. serve에서는 0이 사용 가능한 포트 선택

cli-lan = 이 네트워크와 tailnet의 페어링된 기기가 직접 연결하도록 허용합니다. 종단 간 암호화됩니다

cli-lan_port = --lan 연결용 포트

cli-tailscale_serve = 브라우저용으로 --lan을 Tailscale HTTPS로도 게시합니다

cli-headless = 창 없이 엔진을 실행합니다

cli-overview_status = 작업 공간, 엔진 및 원격 액세스 상태를 표시합니다

cli-session_login = 로그인하고 다음 엔진 시작 시 작업 공간 동기화를 사용합니다

cli-login_no_browser = 이 컴퓨터에서 브라우저를 여는 대신 로그인 링크를 출력합니다

cli-session_logout = 저장된 세션을 제거합니다. 다음 엔진 시작은 로컬 전용입니다

cli-sync = 실행 중인 엔진의 실시간 동기화 상태를 표시합니다

cli-daemon = Zuse를 백그라운드 서비스로 실행합니다

cli-daemon_install = 서비스를 설치하고 활성화한 뒤 시작합니다

cli-daemon_serve = zuse headless 대신 뒤따르는 인수로 zuse serve를 실행합니다

cli-daemon_serve_args = zuse serve의 인수

cli-daemon_uninstall = 서비스를 중지하고 제거합니다

cli-daemon_start = 설치된 서비스를 시작합니다

cli-daemon_stop = 서비스를 중지합니다

cli-daemon_restart = 서비스를 다시 시작합니다

cli-daemon_status = 서비스 상태를 표시합니다

cli-update = 새 릴리스를 확인하고 설치합니다

cli-update_check = 업데이트가 있는지만 표시합니다

cli-appshot = 실행 중인 앱에서 Appshot을 찍습니다

cli-open_url = 열 Zuse 링크

cli-paired_link_file = 연결에 사용할 페어링 링크가 담긴 비공개 파일

cli-desktop_missing = 이곳에는 Zuse 데스크톱 앱이 설치되어 있지 않아 창을 열 수 없습니다. 대신 다음 명령 중 하나를 사용하세요.

cli-status_remote = 원격 액세스
