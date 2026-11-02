# CLI presentation only. Commands, flags, URLs, credentials and MCP output retain their wire spelling.

cli-about = Servir le client web Zuse et le moteur local d’agents

cli-usage = Utilisation

cli-options = Options

cli-commands = Commandes

cli-help = Afficher l’aide

cli-language = Langue de l’interface, ou system

cli-listen = Adresse d’écoute du serveur web

cli-ipc_port = Port RPC local des outils ; 0 choisit un port disponible

cli-data_dir = Dossier des données locales

cli-assets = Dossier des ressources du client web

cli-token = Jeton d’authentification du navigateur

cli-public_origin = Origine HTTPS publique

cli-tls_cert = Chaîne de certificats PEM pour HTTPS direct

cli-tls_key = Clé privée PEM correspondante

cli-relay_origin = Origine du relais public

cli-relay_host_token_file = Fichier d’identifiants de routage du relais

cli-mcp = Servir les outils via stdio en les transférant au moteur local

cli-invalid_arguments = Arguments invalides. Consultez l’utilisation ci-dessous.

cli-unknown_argument = Option ou commande inconnue. Consultez l’utilisation ci-dessous.

cli-missing_argument = Un argument requis manque. Consultez l’utilisation ci-dessous.

cli-relay_requires_credentials = L’accès au relais nécessite son origine et le fichier de jeton hôte

cli-https_requires_keys = HTTPS direct nécessite --tls-cert et --tls-key

cli-https_required = L’écoute hors boucle locale nécessite --tls-cert/--tls-key ou --public-origin pour un proxy inverse HTTPS

cli-create_data_failed = Impossible de créer le dossier des données locales

cli-resolve_data_failed = Impossible de résoudre le dossier des données locales

cli-bind_failed = Impossible d’ouvrir le port RPC des agents ; choisissez un autre --ipc-port

cli-missing_home = HOME ou --data-dir est requis

cli-listening = Les outils des agents écoutent sur 127.0.0.1:{ $port }

cli-open = Ouvrir { $url }
