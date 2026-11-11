# CLI presentation only. Commands, flags, URLs, credentials and MCP output retain their wire spelling.

cli-about = Serve the Zuse browser client and local agent engine

cli-usage = Usage

cli-options = Options

cli-commands = Commands

cli-help = Print help

cli-language = Interface language, or system

cli-listen = Browser server listening address

cli-ipc_port = Loopback RPC port for agent tools; 0 selects an available port

cli-data_dir = Local data directory

cli-assets = Browser client assets directory

cli-token = Browser authentication token

cli-public_origin = Public HTTPS origin

cli-tls_cert = PEM certificate chain for direct HTTPS

cli-tls_key = Matching PEM private key

cli-relay_origin = Public relay origin

cli-relay_host_token_file = Relay routing credential file

cli-mcp = Serve agent tools over stdio, forwarding to the local engine

cli-invalid_arguments = Invalid arguments. Check the usage below.

cli-unknown_argument = Unknown option or command. Check the usage below.

cli-missing_argument = A required argument is missing. Check the usage below.

cli-relay_requires_credentials = Relay access requires its origin and host token file

cli-https_requires_keys = Direct HTTPS requires both --tls-cert and --tls-key

cli-https_required = Non-loopback listening requires --tls-cert/--tls-key or --public-origin for an HTTPS reverse proxy

cli-create_data_failed = Could not create the local data directory

cli-resolve_data_failed = Could not resolve the local data directory

cli-bind_failed = Could not bind the agent RPC port; choose another --ipc-port

cli-missing_home = HOME or --data-dir is required

# $port is a port number, never locale-formatted.
cli-listening = Agent tools listen on 127.0.0.1:{ $port }

# $url is an access link. Preserve the complete URL verbatim.
cli-open = Open { $url }

cli-account = Sign in and link this computer when the server starts

cli-pair = Create a private link for another device

cli-pair_label = Name shown in Connected devices

cli-pair_app = Show the QR code for the Zuse app instead of a browser

cli-login = Sign in to your Zuse account

cli-logout = Sign out of your Zuse account

cli-link = Make this computer available through your Zuse account

cli-link_name = Computer name shown in your account

cli-unlink = Stop making this computer available through your account

cli-status = Show account and network status

cli-server_unavailable = No Zuse server is running on agent port { $port }. Start zuse-server first, or pass --ipc-port.

cli-default_device = New device

cli-browser_link = Browser: { $url }

cli-app_link = Zuse app: { $url }

cli-certificate_warning = Your browser will warn about this computer's private certificate the first time. Continue only on a network you trust.

cli-link_warning = Anyone with this link can use this computer. Unused links expire in ten minutes.

cli-local_only = This link only works on this computer. Start zuse-server with --lan or --account to connect other devices.

cli-sign_in_prompt = Open { $url } and confirm the code { $code }

cli-signed_in = Signed in as { $email }

cli-signed_out = Signed out

cli-sign_in_failed = Sign-in did not finish

cli-linked = Available through your account as { $name } at { $url }

cli-unlinked = This computer is no longer available through your account

cli-status_signed_in = Account: { $email }

cli-status_signed_out = Account: not signed in

cli-status_linked = Computer: { $name } at { $url }

cli-status_unlinked = Computer: not linked to an account

cli-status_no_routes = Network: this computer only

cli-assets_missing = Browser files were not found. Browsers cannot open this server until you pass --assets or set ZUSE_WEB_ASSETS.

cli-invitation_failed = Could not create a connection link
