# Chantier 4 — Horizon en tuiles et climat réduit envoyés par le serveur

Plan validé par l'auteur le 2026-11-18 : climat réduit en 125² sur 16 bits, sol lointain en 512² envoyé une fois, cache disque `client-cache/<empreinte>/` dès maintenant (empreinte avec `generator_version`), contrôle « pas de géologie » dans un test, volumes réels mesurés à l'étape 2. Un commit et un rapport par étape, rien n'est poussé.

Avant l'étape 1, le travail « Interfaces » encore non commité (fenêtre du serveur, client, compte, Paramètres, panneau F3, menu Échap) a été commité à part (`daa81de`), pour que les commits du chantier ne mélangent pas les deux.

## Étape 1 — Tuiles d'horizon, en local

- `nascio-chunk::horizon` : `TileKey` (niveau, i, j), `HorizonVertex` (hauteur au 1/16 m, eau au mètre, nature sol/eau/glace, couleur du sol sRGB), `HorizonTile` (32×32 sommets), `water_shade`, `linear_to_srgb8`. Le niveau L est la grille de pas 16·2^L m, sommet (I, J) en (I·pas, J·pas), lu à la position repliée.
- `nascio-voxel::horizon` : `horizon_tile(terrain, clé)` (même calcul qu'avant : hauteur `height_lod`, eau, glace, bloc du dessus et couleur, en `DetMath`) et `ground_colors` (texture du sol lointain, déplacée depuis le visualiseur, en RGB).
- Visualiseur : `tiles.rs` (`TileStore` : tuiles demandées à leur source, gardées par clé, oubliées quand plus du double du nécessaire est gardé). Source locale pour l'instant (calcul sur le pool des chunks). `horizon.rs` assemble chaque niveau à partir des tuiles, puis maille comme avant ; il n'a plus besoin du terrain, seulement des couleurs de l'eau et de la glace de la table des blocs (`WaterColors`).
- Le centre commun des niveaux n'est plus replié dans [0, 100 000) : il reste un multiple de 512 m, donc de tous les pas, et les sommets tombent sur la grille des tuiles. La couture se comporte comme avant (recentrage au passage).
- Différences visuelles attendues : couleurs du sol passées par sRGB 8 bits (écart d'au plus une demi-unité sRGB), hauteurs au 1/16 m. Au-delà des niveaux 0 et 1, le pas ne divise pas 100 000 m : les deux côtés de la couture n'ont pas la même grille (déjà le cas avant).
- **Limite connue** (consignée dans `decisions.md`) : l'horizon ignore les éditions des joueurs.
- Test `crates/voxel/tests/horizon.rs` : même sommet de part et d'autre de la couture (niveaux 0 et 1), hauteur et eau égales au terrain, nature cohérente, déterminisme. **Écrit mais pas lancé** (consigne : aucun essai avant la fin des chantiers 4 et 5 ; le lancement a été refusé).
- fmt et clippy verts (`--workspace --all-targets`).

## Étapes 2 et 3 — Protocole v8 et serveur

Les étapes 2 (protocole) et 3 (serveur) du plan sont regroupées : les volumes demandés ne se mesurent qu'avec un serveur qui construit les tuiles.

- **Protocole v8** :
  - un octet `UniKind` en tête de chaque flux unidirectionnel (colonne 0, tuile 1, ciel 2) ;
  - `ClientMsg::Tiles { keys }` (64 au plus par message) et `ClientMsg::Sky` (une fois par connexion).
- **Codec des tuiles** (`nascio-protocol::horizon`, format 1), en plans successifs puis zstd :
  - hauteurs au 1/16 m en écarts varint ;
  - niveaux d'eau en écarts ;
  - un octet de nature par sommet ;
  - couleurs des seuls sommets de sol, en écarts d'octets.
  - Tout est vérifié au décodage.
- **`WorldSky`** (`nascio-protocol::sky`) :
  - réglages du monde : horloge, astronomie, gradient thermique, atmosphère. **Pas de graine** : elle permettrait de générer le monde, géologie comprise. **Pas de géotherme non plus.**
  - climat réduit : 125² cellules de 800 m × 22 champs sur 16 bits, chacun sur sa propre plage. Les harmoniques sont envoyées en composantes cartésiennes (X0, A·cos, A·sin), donc moyennées sans souci de phase. S'y ajoutent l'amplitude diurne et la hauteur moyenne de la cellule.
  - sol lointain en 512² RGB.
  - Le tout en postcard + zstd 9.
- **Serveur** (`crates/server/src/horizon.rs`) :
  - `TileCache` : tuiles calculées à la demande sur des fils bloquants, partagées par tous les clients, LRU de `cache_tiles` = 8 192 entrées.
  - `world_sky` : construit au démarrage (0,4 s sur world-42).
  - Envoi des tuiles : une file par joueur (au plus `max_queued_tiles` = 1 024), servie seulement une fois que toutes les colonnes à portée sont parties, sur le même budget de 128 Ko/s, avec une priorité QUIC inférieure à celle des colonnes.
  - `Sky` : envoyé une fois, une seconde demande est journalisée dans `abuse.log`.
  - Une demande de tuiles mal formée (plus de 64 clés, niveau ≥ 8) compte comme une violation.
- **Client** (`nascio-client`) : il lit l'octet de type et produit les événements `Tile` et `Sky` (avec les octets compressés, pour le cache disque). Le visualiseur les ignore jusqu'à l'étape 4.
- **Test `crates/protocol/tests/no_geology.rs`** :
  - les champs du ciel sont du climat (préfixes autorisés, aucun mot de géologie, ni « seed ») ;
  - `WorldSky`, `SkyWorld`, `Field`, `GroundColors`, `HorizonTile` et `HorizonVertex` sont décomposés champ par champ : un nouveau champ casse la compilation du test et force à vérifier ce qu'il révèle ;
  - un sommet encodé ne dépasse pas la taille de ses quatre attributs.
  - Tests unitaires du codec (aller-retour, refus) et du ciel (harmoniques ↔ composantes, quantification sur 16 bits). **Écrits, pas lancés.**

### Volumes mesurés (`netbench horizon out/server/world-42`, 8 sites tirés au hasard, 12 fils)

| | Taille | Remarque |
|---|---|---|
| `WorldSky` | **392 Ko** | climat réduit 302 Ko, sol lointain ≈ 89 Ko ; construit en 0,4 s |
| Horizon complet (5 niveaux) | **341 Ko** en moyenne, 274 tuiles | de 283 à 545 Ko (montagnes) ; 1 274 o par tuile ; 1 136 Ko avant zstd |
| Déplacement de 512 m | **12 Ko**, 12 tuiles | de 6 à 20 Ko |

À 128 Ko/s, une première connexion coûte environ 3 s pour `WorldSky`, puis rien grâce au cache. L'horizon complet prend environ 2,7 s, après les colonnes proches. Un déplacement de 512 m prend environ 0,1 s. C'est nettement moins que l'estimation du plan (0,7 Mo et 35 tuiles par déplacement).

- fmt et clippy verts (`--workspace --all-targets`).

## Étape 4 — Client sans monde local

- En réseau (menu ou `--connect`), le client **ne lit plus aucun monde** : l'argument `world` ne sert plus qu'au vol libre (`--fly`, `--bench`) et à `--generate`. `--fly` et `--connect` ne vont plus ensemble. Un `.exe` lancé n'importe où peut se connecter.
- **Cache disque** `client-cache/g<generator_version>-<empreinte>/` à côté de l'exécutable :
  - `sky-p8.bin`, le `WorldSky` compressé ;
  - `tiles-f1/<niveau>_<i>_<j>.bin`, les tuiles.
  - Le nom inclut `generator_version`, la version du protocole (pour le ciel) et le format des tuiles. Une empreinte de serveur qui ne serait pas purement hexadécimale (128 caractères au plus) désactive le cache, car elle deviendrait un chemin.
- **Connexion** (`remote.rs`) :
  - après la poignée de main, le ciel vient du cache, sinon il est demandé au serveur ;
  - la connexion n'est rendue au menu qu'une fois le ciel reçu (délai porté de 20 à 60 s) ;
  - les tuiles reçues sont écrites dans le cache et passées au magasin de tuiles.
- **Tuiles** (`tiles.rs`, source `Remote`) : une tuile absente est d'abord cherchée dans le cache disque, sinon demandée par lots de 64. Elle est redemandée si elle n'est pas arrivée au bout de 10 s, par exemple si la file du serveur était pleine.
- **À la connexion** (`adopt_remote`), les valeurs du serveur remplacent les valeurs d'attente : réglages du monde (`WorldInfo`), climat réduit (même texture, redimensionnée en 125²), sol lointain, couleurs de l'eau et de la glace, table des blocs, source des tuiles, bloc du mode dieu. Avant la connexion : ciel clair et sol neutre (le menu ne les montre pas).
- **Ce qui lisait le terrain local lit désormais les blocs reçus** :
  - plancher de la caméra : premier bloc solide sous elle ; pas de plancher là où rien n'est encore arrivé ;
  - HUD : hauteur et bloc du sol ;
  - sous terre : profondeur et bloc ; **plus de température de la roche ni d'alerte magma en réseau**, car elles demandent la géologie ;
  - rayon du mode dieu.
  - La téléportation du panneau F3 est grisée en réseau : le serveur refuserait le saut. La graine n'est plus affichée en réseau, puisque le serveur ne la donne pas.
- Le contrôle « table des blocs du serveur = table du monde local » disparaît, puisqu'il n'y a plus de monde local.
- En vol libre, rien ne change : terrain, deltas, climat en 625², tuiles calculées sur place.
- fmt et clippy verts (`--workspace --all-targets`). **Rien n'a été lancé.**

## Étape 5 — Documentation

- Spec : §7, l'horizon en tuiles (dont la limite des éditions) et ce que le client réseau reçoit ; §8.1, la v8 et l'octet de type des flux ; §8.3, les tuiles, `WorldSky`, les volumes mesurés et le cache disque du client.
- `CLAUDE.md` : commandes du client (menu sans monde, `--fly` avec monde) ; `serveur-etat.md` à jour.

## À vérifier par l'auteur (après le chantier 5)
- Rien n'a été lancé à part la mesure `netbench horizon`. Tests écrits mais pas lancés : `crates/voxel/tests/horizon.rs`, `crates/protocol/tests/no_geology.rs`, ainsi que les tests unitaires des codecs. Les tests du serveur (`edit`, `mesh`, `entities`…) passent par `nascio-client`, qui lit maintenant l'octet de type : à relancer.
- À l'écran, en `--fly` puis en réseau : l'horizon est-il identique à l'ancien ? Attendus : couleurs en sRGB 8 bits, centre non replié. Vérifier aussi la couture, le ciel en 125² (neige saisonnière, nuages un peu plus flous), le sol lointain, et une seconde connexion qui doit lire le cache sans rien télécharger.
- Un client lancé hors du dépôt (`.exe` copié ailleurs) doit se connecter sans dossier de monde.

## Tests (2026-11-18, avant le chantier 5)
- `cargo test --workspace` : tout passe du premier coup, y compris `crates/voxel/tests/horizon.rs` (2), `crates/protocol/tests/no_geology.rs` (3), les codecs du protocole (19 tests unitaires) et les tests du serveur (`edit`, `mesh`, `entities`, `security`, `spawn`, `accounts`). Aucune correction n'a été nécessaire.
- Nouveau test `tiles::tests::a_second_connection_reads_the_disk_cache` (visualiseur) : un serveur réel, deux connexions qui partagent un dossier de cache temporaire. La première reçoit `WorldSky` et 6 tuiles. La seconde trouve tout sur le disque : `ensure` aboutit dès le premier appel, sans demande. Les compteurs du serveur (`sent_skies`, `sent_tiles`, ajoutés pour l'occasion) n'ont pas bougé deux secondes plus tard. `Remote::start_in` prend la racine du cache ; `Remote::start` garde `client-cache/` à côté de l'exécutable.
