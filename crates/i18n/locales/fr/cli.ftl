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

cli-server_unavailable = Aucun serveur Zuse ne s'exécute sur le port d'agent { $port }. Démarrez d'abord zuse serve ou indiquez --ipc-port.

cli-default_device = Nouvel appareil

cli-browser_link = Navigateur : { $url }

cli-app_link = Application Zuse : { $url }

cli-certificate_warning = La première fois, votre navigateur signalera le certificat privé de cet ordinateur. Continuez uniquement sur un réseau de confiance.

cli-link_warning = Toute personne disposant de ce lien peut utiliser cet ordinateur. Les liens inutilisés expirent après dix minutes.

cli-local_only = Ce lien ne fonctionne que sur cet ordinateur. Démarrez zuse serve avec --lan ou --account pour connecter d'autres appareils.

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

cli-zuse_about = Contrôleur multi-appareils pour agents de code

cli-arguments = Arguments

cli-version = Afficher la version

cli-engine_port = Port RPC local du moteur, par défaut ZUSE_IPC_PORT ou 27654 ; avec serve, 0 choisit un port disponible

cli-lan = Permettre aux appareils associés de ce réseau et du tailnet de se connecter directement, avec chiffrement de bout en bout

cli-lan_port = Port des connexions --lan

cli-tailscale_serve = Publier aussi --lan en HTTPS via Tailscale pour les navigateurs

cli-headless = Exécuter le moteur sans fenêtre

cli-overview_status = Afficher l’état de l’espace de travail, du moteur et de l’accès à distance

cli-session_login = Se connecter et activer la synchronisation de l’espace de travail au prochain démarrage du moteur

cli-login_no_browser = Afficher un lien de connexion au lieu d’ouvrir un navigateur sur cet ordinateur

cli-session_logout = Supprimer la session enregistrée ; le prochain démarrage du moteur sera uniquement local

cli-sync = Afficher l’état de synchronisation du moteur en cours d’exécution

cli-daemon = Exécuter Zuse comme service en arrière-plan

cli-daemon_install = Installer, activer et démarrer le service

cli-daemon_serve = Exécuter zuse serve avec les arguments qui suivent, au lieu de zuse headless

cli-daemon_serve_args = Arguments pour zuse serve

cli-daemon_uninstall = Arrêter et supprimer le service

cli-daemon_start = Démarrer le service installé

cli-daemon_stop = Arrêter le service

cli-daemon_restart = Redémarrer le service

cli-daemon_status = Afficher l’état du service

cli-update = Rechercher une version plus récente et l’installer

cli-update_check = Indiquer seulement si une mise à jour est disponible

cli-appshot = Prendre un Appshot dans l’application ouverte

cli-open_url = Lien Zuse à ouvrir

cli-paired_link_file = Fichier privé contenant un lien d’association pour se connecter

cli-desktop_missing = L’application de bureau Zuse n’est pas installée ici, aucune fenêtre ne peut donc s’ouvrir. Utilisez plutôt l’une de ces commandes.

cli-status_remote = Accès à distance
