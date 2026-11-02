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
