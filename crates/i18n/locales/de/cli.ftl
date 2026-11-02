# CLI presentation only. Commands, flags, URLs, credentials and MCP output retain their wire spelling.

cli-about = Zuse-Browserclient und lokale Agenten-Engine bereitstellen

cli-usage = Aufruf

cli-options = Optionen

cli-commands = Befehle

cli-help = Hilfe anzeigen

cli-language = Oberflächensprache oder system

cli-listen = Listen-Adresse des Browserservers

cli-ipc_port = Lokaler RPC-Port für Agenten-Tools; 0 wählt einen freien Port

cli-data_dir = Lokales Datenverzeichnis

cli-assets = Verzeichnis der Browserclient-Dateien

cli-token = Browser-Authentifizierungstoken

cli-public_origin = Öffentlicher HTTPS-Ursprung

cli-tls_cert = PEM-Zertifikatskette für direktes HTTPS

cli-tls_key = Zugehöriger privater PEM-Schlüssel

cli-relay_origin = Öffentlicher Relay-Ursprung

cli-relay_host_token_file = Datei mit Relay-Routing-Zugangsdaten

cli-mcp = Agenten-Tools über stdio an die lokale Engine weiterleiten

cli-invalid_arguments = Ungültige Argumente. Siehe Aufruf unten.

cli-unknown_argument = Unbekannte Option oder unbekannter Befehl. Siehe Aufruf unten.

cli-missing_argument = Ein erforderliches Argument fehlt. Siehe Aufruf unten.

cli-relay_requires_credentials = Relay-Zugriff erfordert Ursprung und Host-Token-Datei

cli-https_requires_keys = Direktes HTTPS erfordert --tls-cert und --tls-key

cli-https_required = Nichtlokales Lauschen erfordert --tls-cert/--tls-key oder --public-origin für einen HTTPS-Reverse-Proxy

cli-create_data_failed = Lokales Datenverzeichnis konnte nicht erstellt werden

cli-resolve_data_failed = Lokales Datenverzeichnis konnte nicht aufgelöst werden

cli-bind_failed = Agenten-RPC-Port konnte nicht gebunden werden; anderen --ipc-port wählen

cli-missing_home = HOME oder --data-dir ist erforderlich

cli-listening = Agenten-Tools lauschen auf 127.0.0.1:{ $port }

cli-open = Öffnen { $url }
