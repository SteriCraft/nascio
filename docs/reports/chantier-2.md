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
