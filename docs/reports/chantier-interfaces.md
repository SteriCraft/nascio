# Chantier « Interfaces » — fenêtre du serveur, menu et chat du client

Demande de l'auteur du 2026-11-18. **Rien n'a été lancé ni testé** (consigne : les essais viendront après les chantiers 4 et 5). Seuls `cargo fmt` et `cargo clippy --workspace --all-targets -- -D warnings` ont été passés, sans erreur ni avertissement ; `cargo test` n'a pas été lancé.

## Ce qui a été fait

### Serveur : la fenêtre (`crates/server/src/gui/`)
- Fonctionnalité `gui` de `nascio-server`, active par défaut (Bevy 0.20.1 + bevy_egui 0.43.1, les versions déjà figées). `--nogui` lance le serveur sans fenêtre (console du terminal seule, mesures imprimées toutes les 10 s comme avant). Un serveur sans écran se compile avec `--no-default-features`.
- **Centre : la carte**, déplacée depuis le visualiseur. Elle est **fixe** : le carré du monde est dessiné une fois, sans répétition. Le zoom marche (molette, le point sous le curseur reste en place) et le déplacement s'arrête aux bords. « Vue entière » recadre.
- **Aspect « Visuel » par défaut** (`preview::colorize_visual`, nouveau) : relief ombré teinté par une couleur naturelle par biome, mer plus sombre avec la profondeur, roche nue au-dessus de 2 200 m puis neige au-dessus de 3 600 m, lacs et rivières d'ordre ≥ 5. Sa légende est `preview::visual_legend`.
- **À gauche, zone défilante** : la date (temps réel, curseurs, animation), l'aspect (Visuel, cartes climatiques à la date, couches), la légende, la graticule (désactivée par défaut), le jour/nuit, les sites d'apparition (losanges jaunes). L'infobulle de la carte est reprise.
- **Joueurs sur la carte** : un point rouge par joueur. Le joueur choisi dans la liste est cerclé de jaune, avec son nom au-dessus.
- **En bas, la console**, redimensionnable verticalement. On y voit les lignes du serveur, dont la commande tapée (`> …`), et on tape les commandes sur une ligne en bas (Entrée, flèches haut/bas pour l'historique). La console du terminal reste utilisable.
- **À droite, en haut, les graphiques**, sur les 5 dernières minutes avec un échantillon par seconde : durée du tick (p50, p99, et le budget de 50 ms en pointillé), joueurs, envoi (colonnes et entités, en Kio/s), colonnes générées et éditions par seconde.
- **À droite, en bas, la liste des joueurs** (défilante). Un clic gauche sélectionne un joueur, un nouveau clic le désélectionne. Le clic droit ouvre un menu : « Bannir… » (durée définitive, 1 h, 1 jour, 7 jours ou 30 jours, et une raison facultative) et « Envoyer un message… ».
- **Bouton « Bannis »** en haut à droite de la liste : il ouvre la fenêtre des joueurs bannis. Le bouton « Débannir » reste grisé tant qu'aucun joueur n'est sélectionné.
- La fenêtre ne modifie rien directement : chaque action passe par une commande de la console (`ban`, `unban`, `tell`), qui laisse sa ligne dans la console. Elle se rafraîchit 10 fois par seconde au premier plan, et 2 fois par seconde sinon.
- Fermer la fenêtre arrête le serveur, comme `stop`. Et `stop` ou Ctrl+C ferment la fenêtre.

### Serveur : console, chat, découverte
- `console.rs` : les lignes du serveur sont imprimées et gardées (5 000 lignes) pour la fenêtre (`say!`, `say_err!`).
- Nouvelles commandes `message <texte>` (à tous) et `tell <nom> <texte>` (à un seul joueur). Les joueurs lisent `[Admin]: <texte>`.
- Le serveur diffuse à tous l'arrivée d'un joueur, son départ et son bannissement. Un joueur banni n'a pas de ligne « a quitté ». La console du serveur affiche aussi `<nom> joined/left` et les messages des joueurs.
- Les messages des joueurs sont limités : caractères de contrôle retirés, 256 caractères au plus, et `chat_per_s` = 1 avec une rafale de 5 (configuration TOML). Au-delà, le message est ignoré et une ligne va dans `abuse.log`.
- **Découverte** : le serveur répond par UDP sur `discovery_port` (7778 par défaut, 0 pour couper), à l'adresse de `bind`. Sa réponse donne le port du jeu, `server_id`, le nombre de joueurs et le **certificat**. Il ne répond qu'aux sondes de 1 200 octets, donc la réponse n'est jamais plus grande que la question : pas d'amplification.

### Protocole v7
- `ClientMsg::Chat { text }`, `ServerMsg::Chat(Chat)` avec `Chat` = `Joined`, `Left`, `Banned`, `Say { from, text }` ou `Admin` ; `MAX_CHAT_CHARS` = 256. Le client formule lui-même les lignes dans sa langue.
- Module `nascio_protocol::discovery` (paquets seulement) et `nascio_client::discovery` (`scan`, `query`, bloquants).

### Client (`nascio-viewer`)
- Il s'ouvre sur un **menu** : « Se connecter », « Paramètres » (sans effet pour l'instant) et « Quitter ».
- « Se connecter » ouvre une boîte avec l'adresse (127.0.0.1 par défaut ; `ip`, `ip:port` ou un nom d'hôte) et le bouton **« Scanner le réseau local »** (diffusion et 127.0.0.1, 1,5 s). Un clic sur un serveur trouvé remplit l'adresse. La connexion se fait sur un fil à part, avec une roue d'attente, et les erreurs s'affichent dans la boîte.
- Une fois connecté, la vue 3D existante s'ouvre au point d'apparition donné par le serveur (avant, c'était la position de la caméra).
- **La carte du client est retirée** (`map.rs` supprimé, ainsi que le mode Carte, le bouton « Back to map », `--map-layer` et `--section`). La **minicarte** de la vue 3D est retirée elle aussi, puisque c'était une carte du monde.
- **Chat** : Entrée l'ouvre ; Entrée envoie la ligne, Échap ferme. Les lignes affichées sont « <joueur> a rejoint le serveur », « <joueur> a quitté le serveur », « <joueur> a été banni », `<nom> texte` et `[Admin]: <message>`. Chat fermé, les lignes s'effacent après 10 s. Pendant la saisie, le clavier ne déplace plus la caméra et le mode dieu est suspendu.
- Si le serveur coupe la connexion (expulsion, bannissement, perte), une fenêtre « Déconnecté » donne la raison et propose « Quitter ».
- `--fly` garde le vol libre local sans serveur (développement, benchmark, captures). `--connect <adresse>` se connecte sans passer par le menu.
- Le monde par défaut devient `out/server/world-42`. Le client lit encore le monde local pour le ciel et l'horizon, jusqu'au chantier 4.

### Ménage
Supprimés : `viewer.bat` (suivi par git), `target-viewer/` (2,8 Go) et `target-server/` (382 Mo), qui étaient les dossiers de compilation de ces scripts, puis `out/server/run_server.bat` et `out/server/nascio-server.exe` (l'ancien exécutable, sans fenêtre). `.gitignore` est mis à jour. Je n'ai pas ajouté de nouveau lanceur, voir plus bas.

## Lancer
```powershell
cargo run -p nascio-server --release -- out/server/world-42 --data out/server/data --open-registration
cargo run -p nascio-viewer --release            # menu, monde out/server/world-42
```

## Décisions (aussi dans `decisions.md`)
- **Bevy + bevy_egui pour la fenêtre du serveur**, plutôt qu'eframe : les versions sont déjà figées, et le code de la carte se reprend presque tel quel. L'inconvénient est un binaire plus lourd, d'où la fonctionnalité `gui` et `--nogui`.
- **Certificat donné par la découverte et épinglé à la première connexion** (`out/client/known_servers.toml`, une entrée par adresse) : un certificat différent ensuite est refusé, avec un message. `--cert` force un fichier. Sur un réseau local hostile, la première connexion peut être usurpée (confiance au premier usage). Pour des serveurs publics, il faudra que le site publie l'empreinte : question ouverte.
- La fenêtre agit par des commandes de console, pour une seule logique et une trace dans la console.
- Le choix d'un point d'apparition au clic n'a pas été repris : chaque compte a son point (chantier 3). La carte montre les sites à la place.

## Perdus ou à décider
- La **coupe verticale** (clic droit sur la carte) disparaît avec la carte du visualiseur. L'outil `geology section` l'exporte toujours en PNG. Je peux la remettre dans la fenêtre du serveur si tu le souhaites (il faudrait que le serveur dépende de `nascio-tools`).
- La **spec doit changer** : texte proposé ci-dessous, rien n'est modifié dans `world-spec.md`.
- Pas de lanceur `.bat` : tu avais retiré celui de la racine au profit du dossier autonome `out/server`, et tu demandes maintenant de retirer les lanceurs. Si tu veux un dossier serveur autonome avec le nouvel exécutable, il suffit d'y copier `target/release/nascio-server.exe`.

## Texte proposé pour la spec
**§7** : renommer en « Client (`nascio-viewer`) et fenêtre du serveur ».
- Client : menu (Se connecter, Paramètres, Quitter) ; boîte de connexion avec l'adresse et le scan du réseau local ; vue 3D ; chat (Entrée). Pas de carte ni de minicarte. Le vol libre local reste un outil de développement (`--fly`).
- Fenêtre du serveur : carte **fixe**, sans répétition, avec zoom, aspect « Visuel » par défaut et les autres couches dans la colonne de gauche ; jour/nuit, graticule, sites d'apparition, joueurs en points rouges (le joueur choisi est nommé) ; console redimensionnable en bas ; à droite, graphiques des mesures et liste des joueurs (menu au clic droit : bannir, message ; liste des bannis et « Débannir »).
- Retirer la répétition (UV en `fract`), le choix du point d'apparition au clic, la coupe au clic droit (ou la garder côté serveur) et la minicarte.

**§8.1** : `PROTOCOL_VERSION` vaut 7.

**§8.7 (nouveau) Chat et découverte** : `ClientMsg::Chat`, `ServerMsg::Chat` (arrivée, départ, bannissement, joueur, administrateur ; 256 caractères ; 1 ligne/s, rafale de 5). Console `message` et `tell`. Découverte UDP sur le port 7778 : sonde de 1 200 octets, réponse au plus aussi grande (port, `server_id`, joueurs, certificat) ; le client épingle le certificat à la première connexion.

## Après la revue de l'auteur (2026-11-18)
- Réponses : la coupe verticale reste hors des interfaces ; la spec est mise à jour (§7 réécrit, §8.1 v7, §8.7 nouveau) ; la confiance au premier usage reste une question ouverte (`serveur-etat.md`).
- **Boîte de démarrage du serveur** (`crates/server/src/gui/setup.rs`) :
  - monde existant (dossier) ou nouveau monde (graine, bouton « Aléatoire », résolution, dossier `world-<graine>` à côté de l'exécutable), avec la progression de la génération ;
  - options de la ligne de commande : données, configuration, adresse d'écoute, nom du serveur, point d'apparition commun, inscription ouverte ;
  - choix gardés dans `server-launch.toml` à côté de l'exécutable.
  - Sans argument, `nascio-server.exe` ouvre cette boîte. `--nogui` exige le dossier du monde.
- Commande `seed`. Les joueurs n'ont aucune commande dans le chat.
- Toujours rien de lancé : fmt et clippy verts, et `cargo test` n'a pas été lancé.

## Compte et paramètres du client (2026-11-18)
- Boîte de connexion : **nom du compte et mot de passe** (masqué), sous l'adresse et le scan. Le client refuse d'emblée un nom vide ou un mot de passe de moins de 8 caractères. Avec `--token`, la ligne « Jeton » indique qu'il vient de la ligne de commande.
- **« Paramètres »** (menu) : distance de vue, horizon lointain, ombres, courbure (et rayon, bouton « réelle »), synchronisation verticale, plein écran, magnitude limite des étoiles. Fermer la fenêtre enregistre.
- Fichier `client-settings.toml` à côté de l'exécutable (`settings.rs`) : ces réglages, plus le nom, le mot de passe (en clair) et la dernière adresse, enregistrés après une connexion réussie. La ligne de commande les remplace pour la session sans les écrire ; `--bench` et `--screenshot` ignorent le fichier.
- Le panneau de la vue 3D perd ces réglages et devient un **panneau de débogage, F3** (caché par défaut, ouvert avec `--fly`) : position, mesures, horizon, heure et jour forcés, plancher nocturne, ciel, téléportation.
- `--name` et `--secret` n'ont plus de valeur par défaut (`viewer`) : `--connect` sans nom enregistré demande `--name`.
- **Menu en jeu** (`pause.rs`) : Échap, chat fermé et hors mode dieu, ouvre « Reprendre / Paramètres / Quitter » sur la vue assombrie ; souris libre, caméra immobile, Entrée n'ouvre pas le chat. Échap ou « Reprendre » ferme le menu, et enregistre si les Paramètres ont été ouverts. « Quitter » ferme le client. La fenêtre « Paramètres » est commune au menu d'accueil et au menu en jeu (`settings::settings_window`).
- fmt et clippy verts ; rien de lancé.
