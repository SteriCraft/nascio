# Rapport S3 — Serveur minimal (2026-11-18)

## Fait
- **`crates/server` (`nascio-server`, bibliothèque et exécutable)** :
  - `config` : `ServerConfig` en TOML (adresse, tick, `fsync_interval_ms`, rayon des colonnes, marge d'oubli, budget par client, colonnes en vol, taille du cache, fils de génération, joueurs maximum, point d'apparition, `checkpoint_bytes`) ; une clé inconnue est refusée.
  - `columns` : cache de colonnes compressées, partagé entre tous les clients. Chaque colonne est `generate ⊕ delta` sur la plage du générateur, élargie aux chunks modifiés, puis encodée en codec v2 et compressée en zstd. Une colonne demandée par plusieurs clients en même temps n'est construite qu'une fois. La construction tourne sur des fils bloquants, limités par un sémaphore. Le cache est LRU et une édition invalide la colonne. La version d'une colonne est `region_lsn`, lue **avant** les chunks.
  - `sim` : fil de simulation à 20 Hz. Les entrées sont traitées par identifiant de joueur croissant. `SetBlock` passe par `DeltaStore::set` ; une édition est refusée si le bloc est inconnu ou si la colonne n'est pas chargée par le joueur (`EditRefused`). Les changements sont diffusés en fin de tick, un message `Blocks` par chunk, aux joueurs abonnés à la colonne. Les durées des ticks sont conservées pour les statistiques.
  - `net` : QUIC (`quinn`, TLS par `rustls` et `ring`), certificat auto-signé créé une fois dans `<données>/server/`, que les clients épinglent. Poignée de main (version du protocole, joueurs maximum), `Pong` avec l'heure UTC du serveur, `Move`. Envoi des colonnes : les plus proches d'abord dans le rayon, un flux unidirectionnel par colonne, 16 en vol, budget par client en seau à jetons ; `Forget` au-delà du rayon plus la marge. Un joueur est abonné à une colonne **avant** sa lecture : il ne peut manquer aucun changement.
  - Un fil d'écriture appelle `DeltaStore::flush` toutes les 50 ms (validation groupée). `shutdown` rend tout durable.
  - Exécutable : `nascio-server <monde> [--data <dossier>] [--config <toml>] [--bind <adresse>]`, statistiques toutes les 10 s.
- **`crates/client` (`nascio-client`, sans interface pour l'instant)** : `connect` (épinglage du certificat, poignée de main, tâches de lecture et d'écriture, réception des colonnes) et `Mirror`, qui applique la règle d'ordre des versions. Un lot de `Blocks` est appliqué à une colonne de version inférieure ; il est gardé tant que sa colonne n'est pas arrivée. Au-dessus des chunks d'une colonne, il n'y a que de l'air.
- **Protocole** : la version d'une colonne et le numéro d'un lot `Blocks` deviennent des LSN `u64`. Un seul compresseur zstd par fil.
- **`nascio-voxel`** : `DeltaStore::region_lsn`.
- **`netbot`** (`nascio-tools`) : bots sans interface qui mesurent le temps d'arrivée.
- **Test d'intégration** `crates/server/tests/edit.rs` (monde 1/25 en mémoire) : deux clients reçoivent les colonnes autour du point d'apparition ; A pose un bloc et **A et B le voient** ; une édition hors des colonnes de A est refusée ; un troisième client, arrivé ensuite, reçoit la colonne reconstruite avec l'édition ; **après un redémarrage**, un quatrième client la voit aussi. Mode release : 0,7 s, dont 160 ms pour les 29 colonnes du premier client.

## Mesures (monde d'échelle 1 `out/m7/world-42`, serveur et bots sur le même PC, réglages par défaut)
- Point d'apparition par défaut (50 000, 50 000), en mer : 797 colonnes (rayon de 512 m), 2 391 chunks. Un bot les a toutes en **5,3 s** (la moitié en 2,2 s). Huit bots en même temps : 5,4 à 5,7 s chacun. Chaque colonne n'a été construite qu'une fois (797 construites, 7 173 envoyées, 3,1 Mo) : la limite est le budget de 128 Ko/s par client, pas la génération.
- 430 o par colonne en mer. Sur terre, S0 donne 1 290 o, soit environ 1 Mo et **8 s** pour une arrivée à 128 Ko/s (non mesuré ici : le point d'apparition par défaut est en mer).
- Tick à vide : p99 de 0,04 ms.

## Décisions (détail dans `decisions.md`)
- Version d'une colonne = LSN de sa région, lu avant les chunks ; règle d'application des lots côté client.
- Abonnement à une colonne avant sa lecture.
- Un flux QUIC unidirectionnel par colonne.
- Certificat auto-signé épinglé ; `ring` plutôt qu'`aws-lc-rs`.
- Une crate client sans interface, partagée par le futur client 3D et les bots.
- Le serveur refuse l'édition d'une colonne que le joueur n'a pas. La portée d'un joueur relève du gameplay et n'est pas décidée (S7).

## Limites
- Pas encore d'entités répliquées (S5) : `Move` ne fait que déplacer le centre des colonnes envoyées.
- Les points de reprise du journal se font encore dans le fil de simulation (au pire quelques millisecondes).
- Un client qui creuse sous la plage de sa colonne reçoit des chunks d'air à la place de la roche inconnue (`Mirror`), en attendant le renvoi de la colonne.
- `Have` (le cache disque du client) n'est pas encore traité.

## Questions pour l'auteur
- **Budget par client** : à 128 Ko/s, une arrivée sur terre prend environ 8 s pour 512 m. Faut-il 256 Ko/s par défaut, ou accepter de charger d'abord les colonnes proches ?
- **Point d'apparition** : (50 000, 50 000) par défaut, en mer sur la graine 42. Le choix du point d'apparition relève du gameplay.
- **Portée des éditions** (distance maximale entre le joueur et le bloc) : règle de jeu, à fixer en S7.
