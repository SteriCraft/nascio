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

## Étape 2 — Protocole v8 et serveur

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
