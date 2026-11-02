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
