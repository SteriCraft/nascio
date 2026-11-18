# Chantier 2 — comptes : bannissement, inscription fermée, jetons du site

Réponses de l'auteur (2026-11-18) :
- un jeton est valable 5 minutes et ne sert qu'une fois ;
- la connexion par secret est réservée aux comptes admin (`secret_login = "admin"` par défaut) ;
- le bannissement se fait par compte, temporaire ou permanent, jamais par IP ;
- l'invitation relève du site : le serveur n'a pas de code d'invitation.

Ajout de l'auteur : comme les nonces sont gardés en mémoire, tout jeton émis avant le démarrage du serveur est refusé, en tenant compte de la tolérance d'horloge. Ainsi, un redémarrage n'ouvre pas de fenêtre de rejeu.

## 2.1 — Bannissement et inscription fermée
**Fait :**
- **`accounts.toml`** : deux champs facultatifs par compte, absents du fichier quand ils ne servent pas :
  - `admin = true` ;
  - une table `[account.ban]` avec `since_unix_ms`, `until_unix_ms` (absent si le bannissement est permanent) et `reason`.

  Un bannissement expiré reste écrit, mais il ne compte plus. Les fichiers déjà écrits se lisent sans changement.
- **`Accounts`** : `ban`, `unban`, `banned`, `bans`, `is_admin` et `set_admin`. Chaque changement réécrit le fichier de façon durable, comme `create`. La fonction `Shared::ban` bannit puis expulse le joueur s'il est en ligne ; le message d'expulsion est celui du bannissement.
- **Connexion** : le bannissement est vérifié **après** le secret, comme la règle « déjà connecté », pour qu'un inconnu n'apprenne pas qu'un compte est banni. Le joueur lit par exemple « banned for 30 min: griefing » ou « banned permanently ». Le refus est inscrit dans `abuse.log` (type `login`, « banned »). Il ne compte pas comme un échec dans `LoginGuard`, parce que ce n'est pas une tentative de deviner un secret.
- **Console** :
  - `ban <nom> [durée] [raison]` : la durée s'écrit `30s`, `30m`, `12h`, `7d` ou `2w`. Si le premier mot n'est pas une durée, le bannissement est permanent et ce mot fait partie de la raison ;
  - `unban <nom>` ;
  - `bans` : liste les bannissements en cours, avec le temps restant ;
  - `admin <nom> on|off` : le champ servira en 2.3.
- **Inscription** : `open_registration` vaut désormais faux par défaut. Un nom inconnu reçoit le même refus qu'un mauvais secret (« wrong name or secret »). Pour le développement, `nascio-server --open-registration` rouvre l'inscription : c'est ce qu'il faut pour `swarm` et pour le visualiseur `--connect`, jusqu'aux jetons de la sous-étape 2.3. Les tests d'intégration existants l'activent dans leur configuration.

**Tests :**
- unitaires (`accounts.rs`) : le bannissement est gardé à la réouverture, il expire, le bannissement permanent tient, `unban` fonctionne, le drapeau admin est gardé, les durées sont lues et affichées, et un compte ordinaire ne fait apparaître ni `ban` ni `admin` dans le fichier ;
- intégration (`crates/server/tests/accounts.rs`) :
  - un nom inconnu est refusé et aucun compte n'est créé ;
  - un bannissement de 30 min expulse alice avec sa raison, puis refuse sa connexion ;
  - avec un mauvais secret, alice reçoit seulement « wrong name or secret » ;
  - un bannissement permanent hors ligne refuse bob ;
  - un bannissement expiré ne compte plus ;
  - après `unban`, alice revient ;
  - le refus figure dans `abuse.log`.

**Décisions** (dans `decisions.md`) :
- le bannissement est rangé dans `accounts.toml` et non dans un fichier à part ;
- il est vérifié après le secret ;
- il ne compte pas comme un échec de connexion ;
- `open_registration` vaut faux par défaut, avec une option de développement.

## 2.2 — Jetons signés par le site
**Fait :**
- **Module `nascio_server::token`** :
  - `Claims` contient la version (1), le nom, le serveur (son `server_id`), les dates d'émission et d'expiration (ms UTC) et un nonce de 16 octets ;
  - le jeton s'écrit `base64url(postcard(Claims) ‖ signature Ed25519)`, sans remplissage, et fait environ 140 caractères ;
  - `SiteKey` charge la clé privée (PKCS#8), émet un jeton (`issue`) ou signe des `Claims` arbitraires (`sign`, utilisé par les tests pour forger de mauvais jetons) ;
  - `Verifier` contient la clé publique, le `server_id`, l'instant de démarrage et les nonces vus.
- **Vérification**, dans l'ordre :
  1. la longueur (au plus 512 caractères) et le base64url canonique ;
  2. la signature ;
  3. la version, l'audience et le nom ;
  4. la durée de validité, au plus 5 min, imposée par le serveur et pas seulement par le site ;
  5. l'expiration et l'émission dans le futur, avec 30 s de tolérance ;
  6. **l'émission avant le démarrage + 30 s** ;
  7. le nonce jamais vu.

  Un jeton valide est consommé. Les nonces sont oubliés une fois leur jeton expiré (+ 30 s). Chaque erreur a son message, transmis au client : il ne révèle rien de secret.
- **Règle du redémarrage**, demandée par l'auteur. J'ai pris la borne stricte : `émis ≥ démarrage + 30 s`. En effet, un jeton dépensé juste avant l'arrêt, par un site dont l'horloge avance de 30 s, peut porter jusqu'à cette date ; `démarrage − 30 s` laisserait donc environ une minute de rejeu possible. Le prix est le suivant : avec des horloges synchronisées, les jetons sont refusés pendant les 30 s qui suivent un démarrage, avec le message « token issued before the server started: ask for a new one in 30 s ». Le client devra redemander un jeton. Si c'est gênant, la solution est d'écrire les nonces sur disque : c'est une décision à prendre plus tard.
- **Outil `site`** (`crates/tools/src/bin/site.rs`), qui tient lieu du site :
  - `site keygen <fichier>` écrit la clé privée en base64url et affiche la ligne `site_public_key = "…"` à copier dans la configuration du serveur ;
  - `site public <fichier>` affiche la clé publique d'une clé privée ;
  - `site token <fichier> --name alice --server eu-1 [--ttl 5m]` émet un jeton et refuse une durée de plus de 5 min.

  `nascio-tools` dépend désormais de `nascio-server`, ce qui est permis : `tools` dépend de tout.
- Aucune nouvelle dépendance : le base64url est écrit à la main, en 40 lignes testées.

**Tests unitaires** (`token.rs`) :
- base64url : aller-retour sur 0 à 69 octets, vecteurs connus, refus des caractères `=`, `+` et `/`, des longueurs impossibles et des formes non canoniques ;
- un jeton valide est accepté une fois, puis refusé comme rejoué ; un second jeton pour le même nom passe ; les nonces expirés sont oubliés ;
- refus avec le bon motif : autre clé, octet modifié, texte quelconque, jeton vide ou trop long, version, autre serveur, nom invalide, validité de plus de 5 min, expiré, émis dans le futur ; acceptation aux deux bords de la tolérance ;
- émis avant le démarrage + 30 s : refusé, y compris un jeton frais émis à `T0` ; accepté à partir de `T0 + 30 s` ;
- les clés de mauvaise taille et le `server_id` vide sont refusés.

Essai manuel : `site keygen`, puis `public` (même clé), puis `token` (141 caractères). `--ttl 10m` est refusé.
