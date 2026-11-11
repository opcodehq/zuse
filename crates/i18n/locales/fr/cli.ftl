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

cli-account = Se connecter et lier cet ordinateur au démarrage du serveur

cli-pair = Créer un lien privé pour un autre appareil

cli-pair_label = Nom affiché dans Appareils connectés

cli-pair_app = Afficher le code QR pour l'application Zuse plutôt que pour un navigateur

cli-login = Se connecter à votre compte Zuse

cli-logout = Se déconnecter de votre compte Zuse

cli-link = Rendre cet ordinateur accessible via votre compte Zuse

cli-link_name = Nom de l'ordinateur affiché dans votre compte

cli-unlink = Ne plus rendre cet ordinateur accessible via votre compte

cli-status = Afficher l'état du compte et du réseau

cli-server_unavailable = Aucun serveur Zuse ne s'exécute sur le port d'agent { $port }. Démarrez d'abord zuse-server ou indiquez --ipc-port.

cli-default_device = Nouvel appareil

cli-browser_link = Navigateur : { $url }

cli-app_link = Application Zuse : { $url }

cli-certificate_warning = La première fois, votre navigateur signalera le certificat privé de cet ordinateur. Continuez uniquement sur un réseau de confiance.

cli-link_warning = Toute personne disposant de ce lien peut utiliser cet ordinateur. Les liens inutilisés expirent après dix minutes.

cli-local_only = Ce lien ne fonctionne que sur cet ordinateur. Démarrez zuse-server avec --lan ou --account pour connecter d'autres appareils.

cli-sign_in_prompt = Ouvrez { $url } et confirmez le code { $code }

cli-signed_in = Connecté en tant que { $email }

cli-signed_out = Déconnecté

cli-sign_in_failed = La connexion n'a pas abouti

cli-linked = Accessible via votre compte sous le nom { $name } à l'adresse { $url }

cli-unlinked = Cet ordinateur n'est plus accessible via votre compte

cli-status_signed_in = Compte : { $email }

cli-status_signed_out = Compte : non connecté

cli-status_linked = Ordinateur : { $name } à l'adresse { $url }

cli-status_unlinked = Ordinateur : non lié à un compte

cli-status_no_routes = Réseau : cet ordinateur uniquement

cli-assets_missing = Fichiers du navigateur introuvables. Les navigateurs ne pourront pas ouvrir ce serveur tant que vous n'aurez pas indiqué --assets ou défini ZUSE_WEB_ASSETS.

cli-invitation_failed = Impossible de créer un lien de connexion
