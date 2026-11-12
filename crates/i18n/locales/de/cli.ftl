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

cli-account = Beim Serverstart anmelden und diesen Computer verknüpfen

cli-pair = Privaten Link für ein anderes Gerät erstellen

cli-pair_label = Name unter „Verbundene Geräte“

cli-pair_app = QR-Code für die Zuse-App statt für einen Browser anzeigen

cli-login = Bei deinem Zuse-Konto anmelden

cli-logout = Von deinem Zuse-Konto abmelden

cli-link = Diesen Computer über dein Zuse-Konto verfügbar machen

cli-link_name = Computername in deinem Konto

cli-unlink = Diesen Computer nicht mehr über dein Konto verfügbar machen

cli-status = Konto- und Netzwerkstatus anzeigen

cli-server_unavailable = Auf Agenten-Port { $port } läuft kein Zuse-Server. Starte zuerst zuse serve oder gib --ipc-port an.

cli-default_device = Neues Gerät

cli-browser_link = Browser: { $url }

cli-app_link = Zuse-App: { $url }

cli-certificate_warning = Dein Browser warnt beim ersten Mal vor dem privaten Zertifikat dieses Computers. Fahre nur in einem vertrauenswürdigen Netzwerk fort.

cli-link_warning = Jeder mit diesem Link kann diesen Computer verwenden. Unbenutzte Links laufen nach zehn Minuten ab.

cli-local_only = Dieser Link funktioniert nur auf diesem Computer. Starte zuse serve mit --lan oder --account, um andere Geräte zu verbinden.

cli-sign_in_prompt = Öffne { $url } und bestätige den Code { $code }

cli-signed_in = Angemeldet als { $email }

cli-signed_out = Abgemeldet

cli-sign_in_failed = Die Anmeldung wurde nicht abgeschlossen

cli-linked = Über dein Konto verfügbar als { $name } unter { $url }

cli-unlinked = Dieser Computer ist nicht mehr über dein Konto verfügbar

cli-status_signed_in = Konto: { $email }

cli-status_signed_out = Konto: nicht angemeldet

cli-status_linked = Computer: { $name } unter { $url }

cli-status_unlinked = Computer: nicht mit einem Konto verknüpft

cli-status_no_routes = Netzwerk: nur dieser Computer

cli-assets_missing = Browser-Dateien wurden nicht gefunden. Browser können diesen Server erst öffnen, wenn du --assets angibst oder ZUSE_WEB_ASSETS setzt.

cli-invitation_failed = Verbindungslink konnte nicht erstellt werden

cli-zuse_about = Geräteübergreifende Steuerung für Coding-Agenten

cli-arguments = Argumente

cli-version = Version anzeigen

cli-engine_port = Lokaler RPC-Port der Engine, standardmäßig ZUSE_IPC_PORT oder 27654; bei serve wählt 0 einen freien Port

cli-lan = Gekoppelte Geräte in diesem Netzwerk und Tailnet direkt verbinden lassen, Ende-zu-Ende-verschlüsselt

cli-lan_port = Port für --lan-Verbindungen

cli-tailscale_serve = --lan zusätzlich über Tailscale-HTTPS für Browser veröffentlichen

cli-headless = Engine ohne Fenster ausführen

cli-overview_status = Status von Arbeitsbereich, Engine und Fernzugriff anzeigen

cli-session_login = Anmelden und die Synchronisierung des Arbeitsbereichs beim nächsten Engine-Start einschalten

cli-login_no_browser = Anmeldelink ausgeben, statt auf diesem Computer einen Browser zu öffnen

cli-session_logout = Gespeicherte Sitzung entfernen; der nächste Engine-Start ist nur lokal

cli-sync = Aktuellen Synchronisierungsstatus der laufenden Engine anzeigen

cli-daemon = Zuse als Hintergrunddienst ausführen

cli-daemon_install = Dienst installieren, aktivieren und starten

cli-daemon_serve = zuse serve mit den folgenden Argumenten statt zuse headless ausführen

cli-daemon_serve_args = Argumente für zuse serve

cli-daemon_uninstall = Dienst beenden und entfernen

cli-daemon_start = Installierten Dienst starten

cli-daemon_stop = Dienst beenden

cli-daemon_restart = Dienst neu starten

cli-daemon_status = Dienststatus anzeigen

cli-update = Nach einer neueren Version suchen und sie installieren

cli-update_check = Nur melden, ob ein Update verfügbar ist

cli-appshot = Appshot in der laufenden App aufnehmen

cli-open_url = Zu öffnender Zuse-Link

cli-paired_link_file = Private Datei mit einem Kopplungslink für die Verbindung

cli-desktop_missing = Die Zuse-Desktop-App ist hier nicht installiert, daher kann kein Fenster geöffnet werden. Verwende stattdessen einen dieser Befehle.

cli-status_remote = Fernzugriff
