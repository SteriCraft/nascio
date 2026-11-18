# Rapport d'audit Nascio — avant la phase « Serveur et réseau »

Audit en lecture seule du dépôt `c:\Programmation\git\nascio` (branche `main`, arbre de travail avec les modifications de l'étape 7 laissées intactes), réalisé après la clôture de M6. Référence : section « Audit » de `docs/roadmap.md`.

**Méthode.** Lecture de `CLAUDE.md`, des cinq documents de référence, puis du code (`core`, `worldgen`, `voxel`, `viewer`, shaders `.wesl`). Recherches systématiques (`HashMap`/`HashSet`, `par_iter`/`reduce`, `rand`, `Instant`/`Timestamp::now`, soustractions de coordonnées, espacements de bruit). Exécutions : `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace --release`, `chunkbench` (monde d'échelle 5 et monde d'échelle 1), `topview` (3 sites des anomalies), `nascio-viewer --bench`. Pour localiser les points chauds, j'ai écrit un micro-banc **hors dépôt** (dans le répertoire temporaire de session) qui appelle l'API publique de `nascio-voxel` ; aucun fichier du dépôt n'a été modifié (`git status` identique au départ ; seuls des fichiers dans `out/audit/`, ignoré par git, ont été créés).

**Convention des mesures.** « Mesuré » = obtenu sur cette machine (Windows 11, 12 cœurs, RTX 5060). « Estimé » = calcul ou extrapolation à partir des mesures, non vérifié par une implémentation. Rien n'a été mesuré sur le portable Linux (RTX 3050 Mobile) : voir la section « À mesurer sur le portable ».

---

## 0. Clôture de l'audit (18 novembre 2026)

Tous les constats ont été appliqués en cinq lots, chacun validé par l'auteur, ou classés sans suite pour les points informatifs. La CI est verte sous Linux et Windows sur `c960dad`, le dernier commit de l'audit (`generator_version` 19). Les références de performance mesurées après l'audit figurent dans `docs/roadmap.md`, section « Audit ».

| Constat | Statut | Commit | Ce qui a été fait |
|---|---|---|---|
| E1 roche par bloc | Corrigé | `3d1b780` | Données de roche et bruits calculés une fois par colonne ; sortie identique |
| E2 gauchissement en triple | Corrigé | `3d1b780`, `edd884a` | Gauchissement et biome dédoublonnés, puis table de gradients de Perlin (`generator_version` 17) |
| E3 `DeltaStore` | Corrigé | `1cd5d8f` | Magasin par région verrouillé séparément et chargé à la demande, journal en ajout seul compacté, instantanés par chunk, mémoire bornée ; une édition coûte 0,007 ms au lieu de 4,6 ms |
| E4 `[voxel]` hors empreinte | Corrigé | `71fa31d` | Empreinte = monde + `[voxel]` ; `--dev-override` obligatoire et sorties marquées ; test liant `VoxelConfig::default()` à la version |
| E5 déterminisme entre machines | Corrigé | `71fa31d` | `libm` pour la génération, hachages de référence (monde au 1/25, 24 chunks, coutures), CI Linux et Windows |
| E6 banc sur un monde d'échelle 5 | Corrigé | `71fa31d` | Monde de référence d'échelle 1 `out/m7/world-42`, régénéré en version 19 |
| M1 banquise rectiligne (a) | Corrigé | `28bec13` | Température de l'eau sans relèvement terrestre, seuil tramé |
| M2 chenal gelé sur la banquise (b) | Corrigé | `28bec13` | Embouchures arrêtées à la côte, pas d'eau sur ni sous la glace |
| M3 limite roche / régolithe (c) | Corrigé | `28bec13` | Cause prouvée ; altération amincie et tramée |
| M4 rivières d'un bloc (d) | Corrigé | `28bec13` | Gabarit de jeu par ordre de Strahler, méandres, vallées sans gorges |
| M5 maillage à coût fixe | Corrigé | `581fa00` | Sortie anticipée pour tout chunk uniforme, faces par masques de bits : 2,63 → 0,27 ms par chunk, même maillage |
| M6 mise à l'échelle multi-cœurs | Mesuré, classé | `3d1b780` | Pools d'allocation mesurés puis écartés (sans gain) ; ×6,1 à ×6,9 sur 12 cœurs ; cause probable : l'hyperthreading (non vérifié) |
| M7 `worldgen` monolithique | Corrigé | `8414b89` | Crate de lecture `nascio-world`, dont `voxel` dépend seule |
| M8 outils dans `voxel` | Corrigé | `8414b89` | Binaires dans `nascio-tools` ; `clap` et `image` hors de `voxel` |
| M9 `Terrain` et chemins hérités | Corrigé | `8414b89` | Mondes complets exigés, chemins `Option` retirés |
| M10 shaders | Corrigé | `8160aff` | Voir R1 à R3 |
| M11-1 climat, biomes, glaciers | Corrigé | `61a3a44` | `crates/voxel/tests/climate_path.rs` (1/5 à chaque test, échelle 1 en option) |
| M11-2 `proptest` des deltas | Corrigé | `1cd5d8f` | Propriétés à travers la couture |
| M11-3 hachages de chunks | Corrigé | `71fa31d` | Voir E5 |
| M11-4 chunk décalé d'une période | Corrigé | `71fa31d` | Test `chunk_one_period_away_is_identical` |
| M11-5 `chunk_delta` du visualiseur | Corrigé | `8414b89` | `torus::delta_chunks` dans `core` |
| F1 différence torique locale | Corrigé | `8414b89` | Voir M11-5 |
| F2 diviseurs recalculés | Corrigé | `8414b89` | Mis en cache |
| F3 normales de la courbure | Corrigé | `61a3a44` | Décision marquée comme remplacée (normales plates) |
| F4 nombres magiques de l'alpha | Corrigé | `8160aff` | Canal de matériau en UV0, constantes vérifiées par un test |
| F5 chunk non compressé | Corrigé | `581fa00` | Stockage uniforme et codec palette + plages |
| F6 cache de colonnes non borné | Corrigé | `1cd5d8f` | `ColumnCache` borné |
| F7 CI Linux seulement | Corrigé | `71fa31d` | Tâche Windows ajoutée |
| F8 étape 7 non validée | Corrigé | `9aeb9ad` | Commitée avant le lot 1 |
| F9 `HashSet` de l'horizon | Corrigé | `61a3a44` | `BTreeSet` |
| R1 `local_sun` par pixel | Corrigé | `8160aff` | Forme vectorielle : ni `atan2` ni second `asin` hors de la bande de la couture |
| R2 code d'atmosphère dupliqué | Corrigé | `8160aff` | Modules WESL partagés (`atmosphere.wesl`, `sun.wesl`), portage Rust testé contre `core::astro` |
| R3 étiquettes dans l'alpha | Corrigé | `8160aff` | Voir F4 |
| R4 `hf` calculé deux fois | Corrigé | `8160aff` | Calculé une fois |
| R5 aurores | Ouvert | — | Coût à mesurer sur le portable Linux, avec les prochains bancs |
| R6, R7 | Information | — | Sans action |
| Dette (§6) : relaxation de talus | Close | `61a3a44` | Décision marquée comme retirée (l'érosion la remplace) |
| Spec (§10) : hydraulique, glace de mer | Corrigé | `28bec13` | Gabarit de jeu et banquise dans `world-spec.md` |

**Ajouts de l'auteur pendant l'audit :**
- **Ligne verticale du site polaire (86 592, 0)** — corrigée dans `1769339` (`generator_version` 18). Gradin de 2,5 à 3 m le long d'une ligne de nœuds de la grille d'altitude. La bicubique, bornée après coup par les quatre nœuds de chaque cellule, était écrêtée différemment des deux côtés de la ligne. Elle est remplacée par une bicubique monotone (passes d'Hermite à tangentes limitées).
- **Climat de la couture** — recalé dans `c960dad` (`generator_version` 19). Sur la banquise pérenne, la surface suit l'air au lieu du plancher de l'eau de mer. La moyenne zonale passe de −9,8 à −18,8 °C, pour une cible d'environ −20 °C, et son test est actif.

**Mesures restantes sur le portable Linux (§9)** : les bancs des sous-lots 5b et 5c y ont été faits (écarts dans le bruit). Un nouveau banc complet, aurores comprises (R5), est reporté à la demande de l'auteur.

## 1. Résumé

**État général : bon.** Les invariants de `CLAUDE.md` sont respectés partout où je les ai vérifiés (tore, bruit périodique, RNG par étape, déterminisme, temps UTC). `fmt`, `clippy -D warnings` et la suite de tests (tous les binaires de test) passent sans échec. Le sens des dépendances `core ← worldgen ← voxel ← viewer` est respecté et `core` ne dépend d'aucun moteur graphique (`serde`, `jiff`, `thiserror` seulement).

**Aucun défaut critique** (aucune violation d'invariant ni corruption de données constatée). Les risques se concentrent sur trois sujets :

1. **Performance de génération des chunks** : ~78 % du coût d'une colonne (`ColumnSet::new`) est un seul appel (`Terrain::column_geo`) qui recalcule trois fois les mêmes bruits, et la génération d'un chunk de roche coûte ~10 ms parce que des bruits constants sur la verticale sont recalculés pour chaque bloc. Gain estimé : 3 à 5× sur la génération, sans changer la sortie si l'on conserve les mêmes formules.
2. **Préparation au serveur** : le `DeltaStore` ne passe pas à l'échelle (copie complète à chaque édition pendant qu'un instantané existe, réécriture de région entière, chargement de tout au démarrage), la configuration `[voxel]` n'est ni dans l'empreinte du monde ni dans la version du générateur, et le déterminisme flottant entre machines n'est ni testé ni garanti.
3. **Les quatre anomalies de M6 ont toutes une cause identifiée dans le code ou les données** (section 8) ; aucune n'est un bogue de couture ou de chunk.

Comptes : **0 critique, 6 élevés, 11 moyens, 9 faibles** (détail section 2).

Constat important sur le banc : le monde `out/m6/world-42` est en **échelle 5** (grille tectonique 125×125 de 800 m, grille fine 625×625 de 160 m), et non à la résolution de la spec (32 m). Les références de la feuille de route (65 colonnes/s, etc.) viennent de ce monde. Je les ai reproduites (64,9 col/s) et vérifiées sur un monde d'échelle 1 (`out/pre-m6/world-42`, générateur v10) : 60,9 col/s, donc l'ordre de grandeur tient, mais la mesure officielle devrait se faire sur un monde d'échelle 1.

---

## 2. Constats priorisés

### Critique

Aucun.

### Élevé

**E1 — Génération de roche : bruits constants sur la verticale recalculés pour chaque bloc.**
- Où : `crates/voxel/src/geology.rs:230` (`strata.sample(x, z)`), `:234` (`facies.sample(x, z)`), `:264` et `:268` (`pluton.sample` deux fois) ; appelées par bloc via `Terrain::rock_block` (`crates/voxel/src/terrain.rs:776`), elle-même appelée par `ColumnSet::block` (`crates/voxel/src/generate.rs:233`) pour chacun des 34³ = 39 304 blocs de `padded_chunk` (`:274`).
- Preuve (mesuré) : `rock_block` à 50 m sous la surface coûte 187 à 342 ns, contre 9 ns à la surface (sol seul). Un chunk de surface rocheux coûte **9,6 à 13,6 ms** en `padded_chunk` ; un chunk entièrement d'air ou d'eau coûte 0,08 ms. Un `Fbm` de 2 octaves coûte 173 ns, une octave de Perlin 86 ns (`noise.rs:62` calcule `cos` et `sin` à chaque nœud).
- Impact : c'est le poste dominant de la génération « par chunk » (4,3 ms en moyenne dans `chunkbench`, tirée vers le bas par les chunks d'air). Pour le serveur, un chunk souterrain plein coûte autant qu'un chunk de surface (jusqu'à 91 chunks sous une colonne jusqu'au Moho).
- Correctif : calculer une fois par colonne (dans `ColumnSet::new`) les valeurs indépendantes de `y` : `strata`, `facies`, `pluton` (les deux), l'épaisseur de sol, la glace/neige du glacier, les limites de couche (`weathering`, `sediment`). `rock_block` ne fait alors que des comparaisons d'entiers. Gain estimé : coût par bloc rocheux de ~250 ns à ~30 ns ; chunk de surface de ~10 ms à ~1,5-2 ms (non mesuré). Les sorties restent identiques si les formules sont inchangées (test : comparer les hachages de chunks avant et après).

**E2 — `column_geo` recalcule trois fois le même gauchissement.**
- Où : `crates/voxel/src/terrain.rs:443-466` et `crates/voxel/src/geology.rs:174-205`. `Geology::column` calcule `warp_x`/`warp_z` (2 `Fbm` de 3 octaves), puis appelle `biome()` qui les recalcule (et ajoute `biome_warp_x/z`) ; `column_geo` appelle ensuite `g.biome(...)` une seconde fois avec la position tramée (`terrain.rs:462`), qui recalcule encore les quatre bruits. Le premier résultat de biome est donc jeté.
- Preuve (mesuré) : `column_geo` = 2,37 µs par appel ; `height` = 0,45 µs ; `river` = 30 ns ; `water_top` = 55 ns. `ColumnSet::new` = 3,49 ms = 1296 hauteurs (0,58 ms) + 1156 `column_geo` (**2,74 ms, 78 %**) + plants (0,12 ms) + reste. Recomposé à partir des coûts de bruit : 517 ns (warp) + 863 ns (premier biome, perdu) + 863 ns (biome tramé) = 2,24 µs.
- Impact : ~80 % du coût fixe de chaque colonne, donc aussi du temps de réponse à la première requête d'un joueur.
- Correctif : (1) calculer le warp une fois et le passer à `biome` ; (2) ne pas calculer le biome non tramé quand la trame est active ; (3) tabuler les gradients de Perlin (par exemple 256 directions précalculées, indexées par le hachage) au lieu de `cos`/`sin` par nœud. Gain estimé : (1)+(2) ramènent `column_geo` à ~0,9 µs (`ColumnSet::new` ≈ 1,9 ms) ; (3) divise en plus le coût des bruits par ~3 (`ColumnSet::new` ≈ 0,8-1 ms, estimé). Attention : (3) change les valeurs du bruit, donc `generator_version` doit être incrémentée.

**E3 — `DeltaStore` n'est pas prêt pour un serveur.**
- Où : `crates/voxel/src/delta.rs:198-350`, `crates/viewer/src/god.rs:212` (`Arc::make_mut`), `crates/viewer/src/fly.rs:195-210`.
- Constats (lecture du code, non mesuré) : (a) tout le magasin est un seul `BTreeMap` en mémoire ; chaque édition alors qu'un instantané `Arc` est tenu par un fil de génération **copie le magasin entier** ; (b) `save` réécrit la région entière (25×25 colonnes de chunks, toutes hauteurs) à chaque sauvegarde, de façon synchrone ; (c) `open` lit **toutes** les régions au démarrage ; (d) pas de verrou par région, pas de journal (une coupure entre deux sauvegardes perd les éditions, le renommage atomique protège seulement un fichier).
- Impact : avec plusieurs joueurs qui construisent, coût des éditions en O(taille totale des deltas), latences et mémoire non bornées.
- Correctif : un magasin par région (`RwLock` ou acteur par région), chargement paresseux, journal append-only (WAL) avec compactage périodique, instantanés par chunk (clone d'un seul `ChunkDelta`) plutôt que du magasin entier. Le format de fichier (valeurs absolues, noms de blocs, version du générateur, empreinte du monde) est une bonne base à garder.

**E4 — La configuration de génération des chunks (`[voxel]`) échappe à la version et à l'empreinte du monde.**
- Où : `crates/viewer/src/main.rs:168-169` (`VoxelConfig::from_toml` depuis un fichier optionnel, sinon `default()`), `crates/voxel/src/bin/chunkbench.rs:105` (toujours `default()`), `World::hash` (`crates/worldgen/src/world_io.rs:311`, couches et fichiers seulement), `GENERATOR_VERSION` (`crates/worldgen/src/config.rs:13`).
- Impact : changer une valeur de `VoxelConfig::default()` (épaisseurs de sol, tables de végétation, seuils de glace) ou lancer le serveur avec un autre TOML change les chunks générés sans que l'empreinte ni la version ne bougent ; les deltas en valeur absolue restent valides mais les bords entre blocs modifiés et blocs régénérés deviennent incohérents. Violation de l'esprit de l'invariant 5 (le serveur est la seule source de génération).
- Correctif : enregistrer `[voxel]` dans `world.toml` (ou un fichier du monde), l'inclure dans l'empreinte, et refuser tout autre `VoxelConfig` côté serveur. Mettre un test qui compare la valeur par défaut à un hachage.

**E5 — Déterminisme flottant entre machines : ni testé ni garanti.**
- Où : tout le code `f64` qui appelle `sin`, `cos`, `powf`, `exp`, `atan2`, par exemple `crates/worldgen/src/noise.rs:61-62`, `crates/worldgen/src/climate.rs`, `crates/voxel/src/geology.rs`. Rust délègue ces fonctions à la bibliothèque mathématique de la plateforme (différente entre Windows/MSVC et Linux/glibc, et entre versions).
- Constat : le test de déterminisme (`crates/worldgen/tests/determinism.rs`) compare des exécutions sur la **même** machine (1 fil contre 8 fils) et seulement à l'échelle 25 par défaut (échelle 5 en `#[ignore]`, échelle 1 jamais). Il n'existe pas de hachage de référence (« golden ») ni pour le monde ni pour des chunks. La CI tourne sous Linux seulement (`.github/workflows`), l'auteur sous Windows : une divergence n'apparaîtrait nulle part.
- Impact : le journal de décisions dit que le déterminisme flottant entre machines n'est pas nécessaire parce que seul le serveur génère. C'est vrai tant que le monde est généré une fois et copié ; mais (1) le serveur pourra changer de machine ou de système, (2) la génération des chunks à partir des couches est refaite à chaque démarrage : une différence d'un bit sur la hauteur change un bloc, donc des bords entre chunks déjà modifiés et chunks neufs.
- Correctif : (a) ajouter des hachages de référence (monde échelle 25 et une vingtaine de chunks à des endroits fixes, dont les coutures) comparés en CI Linux **et** en local Windows ; (b) pour les bruits, remplacer `cos`/`sin` par une table (voir E2), ce qui supprime aussi la dépendance à la libm ; (c) documenter que le monde (couches) est l'artefact de référence, à copier et non à régénérer sur chaque machine.

**E6 — Le banc de référence est un monde d'échelle 5, et il manque des mesures sur la cible.**
- Où : `docs/roadmap.md:111`, `docs/decisions.md` (benchmarks), `bench/default.toml:1-3` (l'en-tête demande un monde d'échelle 1).
- Constat : `out/m6/world-42` est en échelle 5 (`world.toml : scale = 5`). Le banc du visualiseur y donne (mesuré, 1920×1080) : moyenne 3,56 ms, p95 4,29 ms, p99 4,98 ms, 1 % low 145,6 i/s, 8 images au-delà de 16,7 ms, CPU 2,33 ms, GPU 1,03 ms. Les ordres de grandeur de `chunkbench` sont proches en échelle 1 (voir section 4), mais la différence de grille change le nombre d'octaves de détail ajoutées par les chunks, donc le coût de `height`.
- Correctif : régénérer un monde d'échelle 1 à la version courante (~140 s) pour tous les bancs officiels et l'ajouter aux scripts d'audit.

### Moyen

**M1 — Anomalie (a), limite de banquise rectiligne.** Voir section 8.
**M2 — Anomalie (b), chenal gelé sur la banquise.** Voir section 8.
**M3 — Anomalie (c), limite roche nue / régolithe.** Voir section 8.
**M4 — Anomalie (d), rivières d'un seul bloc d'eau.** Voir section 8.

**M5 — Maillage : coût fixe de 2,6 ms par chunk, sans sortie anticipée pour les fluides.**
- Où : `crates/voxel/src/mesh.rs:118-212`. `mesh_chunk` ne sort tôt que pour un chunk entièrement d'air ou entièrement solide (`:120`) ; sinon il parcourt 6 directions × 32 tranches × 1024 cellules = 196 608 faces candidates avec deux `get` et des consultations de registre chacune, plus 12 lectures pour l'occlusion de chaque face visible.
- Preuve (mesuré) : un chunk entièrement d'eau (0 quad) coûte 2,56 ms à mailler, comme un chunk de surface à 243-455 quads (2,7 ms) : le coût est essentiellement fixe. Le visualiseur évite le problème avec `ColumnSet::may_have_faces` (`generate.rs:210`), mais `mesh_chunk` lui-même n'est pas protégé.
- Correctif : (1) sortie anticipée pour tout chunk uniforme (même fluide) ; (2) maillage par masques binaires (un `u32` par ligne de 32 blocs solides/opaques, faces par opérations de bits), généralement 5 à 10× plus rapide que la boucle par cellule (estimé) ; (3) réutiliser le tableau `mask` entre appels. Sans effet sur le serveur (il ne maille pas), mais utile pour le client et les bancs.

**M6 — Mise à l'échelle multi-cœurs imparfaite.** `chunkbench` : ×7,68 sur 12 cœurs (échelle 5), ×7,33 (échelle 1) ; le temps CPU par chunk monte de 4,30 à 6,72 ms (+56 %) avec 12 fils. Causes probables (non vérifiées) : hyperthreading, allocations de ~78 Ko par `PaddedChunk` et 64 Ko par `Chunk`, tableaux `Vec<Vec<(i32, Part)>>` par colonne (`generate.rs:40,88`, 1156 `Vec` par colonne). Correctif : réutiliser les tampons par fil (pool d'objets), remplacer `plants` par un tableau plat trié.

**M7 — Architecture : `nascio-worldgen` est monolithique pour un serveur.** `voxel` dépend de tout `worldgen` (tectonique, érosion, climat, `preview.rs` 1 230 lignes, `plot.rs`, `progress.rs`, dépendance `image`) alors qu'il n'utilise que `grid`, `noise`, `rng`, `world_io`, `biomes`, `deposits` (lecture), `blocks`, `erosion::upsample::Detail`. Voir section 5.

**M8 — `nascio-voxel` embarque des outils dans ses dépendances normales.** `clap` et `image` sont des dépendances normales de la crate (`crates/voxel/Cargo.toml`, `image` y est passée en dépendance normale dans les modifications non validées de l'étape 7) à cause des binaires `chunkbench`, `topview`, `geology`. Un serveur qui dépend de `nascio-voxel` les tirerait. Correctif : crate `nascio-tools` (ou `[[bin]]` avec `required-features = ["tools"]`).

**M9 — Magasin de blocs et `Terrain` : « objet Dieu » et chemins hérités.** `crates/voxel/src/terrain.rs` (1 047 lignes) mêle configuration, échantillonnage de hauteur, sols, glaciers, eau, végétation, couleurs, teinte de maillage ; 7 endroits portent des chemins pour des « mondes d'avant M3/M4/M5 » (`Option<Geology>`, `Option<SurfaceClimate>`, `Option<Rivers>`, altitude tectonique de repli, `granite` partout). Ils n'ont plus d'utilité (le serveur ne servira que des mondes complets) et doublent les cas à tester. Correctif : exiger les couches au chargement et retirer les `Option`.

**M10 — Shaders : coût par pixel et duplication.** Voir section 7 (`local_sun` par fragment, code d'atmosphère dupliqué dans `sky.wesl`/`curvature.wesl`, étiquettes magiques dans l'alpha des sommets, pas de test GPU/CPU pour `local_sun`).

**M11 — Trous de test.** Les tests de couture sont nombreux (47 occurrences) mais : (1) le chemin climat/biomes/glaciers n'est couvert que par un monde généré à petite échelle (`crates/voxel/tests/geology.rs`) ; (2) aucune propriété (`proptest`) sur `DeltaStore` à travers la couture ; (3) pas de hachage de référence pour les chunks (voir E5) ; (4) aucun test qui vérifie qu'un chunk tiré à la périphérie de la couture et sa version décalée d'une période sont identiques ; (5) `fly.rs` implémente sa propre différence de chunks (`chunk_delta`, voir F1).

### Faible

- **F1** — `crates/viewer/src/fly.rs:309` réimplémente la différence torique sur les chunks (`chunk_delta`) au lieu de passer par `nascio_core::torus` (une fonction `delta_chunk` ou `ChunkPos::delta` dans `core`). Correct, mais contraire à l'invariant 1 dans la lettre, et le serveur en aura besoin.
- **F2** — `crates/worldgen/src/noise.rs:9` : `world_divisors()` balaie 100 000 entiers à chaque `divisor_at_most` (appelée à la construction de chaque `Fbm`) ; sans conséquence tant que les `Fbm` sont construits une fois, à mettre en cache sinon.
- **F3** — `docs/decisions.md` (courbure d'affichage) dit que les normales sont « corrigées » ; `curvature.wesl:3-5` dit qu'elles restent celles du monde plat (ce qui est cohérent avec l'éclairage voulu). Mettre la décision à jour.
- **F4** — Numéros magiques dupliqués entre `mesh.rs` et `curvature.wesl` : 0,4, 0,97, 0,57, 0,99, 0,995, 0,999 (étiquettes d'eau et de feuillage dans l'alpha). Fragile ; passer par un attribut de sommet dédié ou une constante partagée générée.
- **F5** — `Chunk` stocke 32 768 `u16` sans compression ni notion de chunk uniforme (`crates/voxel/src/chunk.rs`) : 64 Ko par chunk en mémoire et sur le réseau. À traiter avec le protocole (palette + RLE).
- **F6** — `generate.rs:28` : `ColumnSet` (clonable) conserve `bodies: Vec<Body>` et `plants: Vec<Vec<...>>` ; pas de borne mémoire pour un cache de colonnes côté serveur.
- **F7** — CI Linux seulement ; Windows testé localement (`.github/workflows`). Ajouter une tâche Windows ou un hachage de référence partagé (voir E5).
- **F8** — `topview.rs` et la modification de `Cargo.toml` de l'étape 7 ne sont pas validés (arbre non propre) ; à committer avant l'audit de suivi.
- **F9** — `crates/viewer/src/horizon.rs:14` utilise `HashSet` pour le trou de niveau 0 : sans conséquence (appartenance seulement, résultat visuel), mais à noter si l'ordre d'itération est un jour utilisé.

---

## 3. Déterminisme et invariants de `CLAUDE.md` (périmètre 1)

| Invariant | Vérification | Résultat |
|---|---|---|
| Tore partout | Recherche de soustractions de coordonnées brutes dans `crates/{worldgen,voxel,viewer}`. Les différences passent par `torus::delta`, `delta_f64`, `distance_f64`, `delta2` (par exemple `rivers.rs:253`, `deposits.rs:366-367`, `deposits.rs:954`, `fly.rs:121`). Les soustractions restantes sont des écarts de hauteur ou de petites mailles locales (`fly.rs:816`, `section.rs:145`, `generate.rs:428` après `ox` dans un même chunk). | Conforme, à une exception : `fly.rs:309` (`chunk_delta`, F1). |
| Entiers pour les blocs, rendu relatif à la caméra | `BlockPos`/`ChunkPos` en `i32`, `WorldPos` en `f64` ; `fly.rs:121-123` envoie `delta(cam, x) as f32` ; `curvature.wesl` travaille en position relative (`curved.drop.yw`). | Conforme. |
| RNG explicite par étape | `crates/worldgen/src/rng.rs` : `stage_seed` = blake3(« nascio-stage », graine, étiquette) → `ChaCha8Rng` ; `mix64` pour les hachages. Aucun `thread_rng`, `from_entropy` ni `rand::random` dans le dépôt. | Conforme. |
| Pas d'itération `HashMap` qui influence un résultat | `HashMap`/`HashSet` n'existent que dans le viewer (`fly.rs:235`, `god.rs:36`, `horizon.rs:130-218`) pour des caches et des ensembles d'appartenance ; `worldgen`/`voxel` utilisent `BTreeMap`/`BTreeSet` (`deposits.rs`, `delta.rs`). | Conforme. |
| Pas de parallélisme dépendant de l'ordre | Tous les `par_iter`/`into_par_iter` de `worldgen` (`climate.rs`, `tectonics/*.rs`, `erosion/flow.rs:162`, `geology.rs`, `deposits.rs:798`) produisent des `collect` indexés ; les sommes de flottants (`process.rs:122,130`, `climate.rs`) sont séquentielles. Le seul `reduce` est celui de `chunkbench.rs:140` (compteurs entiers). Test `determinism.rs` : 1 fil contre 8 fils, mêmes hachages. | Conforme (voir E5 pour le cas inter-machines). |
| Bruit périodique | `noise.rs:36-44` : `PeriodicNoise::new` **panique** si l'espacement ne divise pas 100 000 ; `Fbm::new` ajuste vers un diviseur (`divisor_at_most`) ; les nœuds sont hachés modulo le nombre de nœuds (`:56-57`). Espacements utilisés : 800, 250, 1 600→1 250, 2 000, 4 000 (voxel) ; test `periodic` (proptest) sur 6 périodes. | Conforme. Remarque : `Fbm::new(…, 1_600.0, …)` est ajusté silencieusement à 1 250 m ; cohérent avec la spec mais à connaître. |
| Temps dérivé de l'UTC | `Timestamp::now` n'apparaît qu'en `crates/core/src/time.rs:41` (`GameClock::now`) ; `Instant::now` ne sert qu'aux durées affichées (`generate.rs`, `erosion/mod.rs:288`, `progress.rs`, `perf.rs`, bancs). | Conforme. |
| Le serveur seul génère | Le viewer génère localement comme outil de développement (documenté). | Conforme pour l'instant ; voir E4. |

Bilan outils : `cargo fmt --check` propre ; `cargo clippy --workspace -- -D warnings` sans avertissement ; `cargo test --workspace --release` sans échec (aucune ligne « FAILED » ; un test ignoré, la génération d'échelle 5). Lints du dépôt : `unsafe_code = forbid`, aucun `unsafe` trouvé.

Couverture de la couture (lue dans le code) : `torus.rs` (11 tests dont proptest), `grid.rs` (accès enveloppés, bilinéaire et bicubique continus à travers la couture, proptest), `noise.rs` (continuité et périodicité), `terrain.rs` (hauteur continue et périodique, lac à cheval sur la couture), `generate.rs` (`borders_agree_across_seams` en x et z, Moho continu), `rivers.rs` (rivière traversant la couture x), `tests/geology.rs` (géologie et minerais continus). Lacunes : voir M11.

---

## 4. Performance de la génération et du maillage (périmètre 2)

### 4.1 Mesures (cette machine)

`chunkbench` (8 sites de 4×4 colonnes, 12 cœurs disponibles) :

| Monde | Fils | Colonnes/s | CPU par colonne | Génération par chunk | Maillage par chunk |
|---|---|---|---|---|---|
| Échelle 5 (`out/m6/world-42`, v14) | 1 | 64,9 | 3,62 ms | 4,30 ms | 2,66 ms |
| Échelle 5 | 12 | 498,1 (×7,68) | 5,68 ms | 6,72 ms | 3,07 ms |
| Échelle 1 (`out/pre-m6/world-42`, v10) | 1 | 60,9 | 3,30 ms | 5,13 ms | 2,59 ms |
| Échelle 1 | 12 | 446,2 (×7,33) | 5,41 ms | 8,93 ms | 3,06 ms |

La référence de la feuille de route (65 / 524 colonnes/s) est reproduite à 1 % près sur 1 fil et à 5 % sur 12 fils. Remarque : en moyenne 1,7 chunk par colonne dans ce banc (seuls les chunks de surface sont comptés).

Micro-mesures (monde d'échelle 5, banc hors dépôt, appels isolés sur 20 000 positions) :

| Fonction | Coût |
|---|---|
| `Terrain::height` | 450 ns (dont `river` 30 ns) |
| `Terrain::column_geo` | **2 370 ns** |
| `Terrain::water_top` | 55 ns |
| `Terrain::climate` | 58 ns |
| `Terrain::moho_y` | 22 ns |
| `rock_block` à la surface / à 50 m | 9 ns / 187-342 ns |
| `plants_in` (zone 45×45 m) | ~120 µs |
| `ColumnSet::new` | 3,49-3,52 ms |
| `padded_chunk`, chunk rocheux de surface | 7,7-13,6 ms |
| `padded_chunk`, chunk d'air ou d'eau | 0,08 ms |
| `mesh_chunk`, tout chunk non uniforme (même 0 quad) | 2,56-2,72 ms |
| Perlin, 1 octave / `Fbm` 2 octaves / 3 octaves | 86 / 173 / 259 ns |

Décomposition de `ColumnSet::new` : hauteurs 1296 × 0,45 = 0,58 ms (17 %) ; `column_geo` 1156 × 2,37 = 2,74 ms (78 %) ; plantes 0,12 ms ; eau, Moho, pentes : ~0,1 ms. La somme (3,5 ms) correspond à la mesure.

Banc du visualiseur (`bench/default.toml`, échelle 5, 1920×1080, ombres actives, vsync off) : voir E6. Les 8 images dépassant 16,7 ms (0,02 %) et le maximum de 29,4 ms (56 ms lors du banc de référence de M6) viennent probablement du chargement de colonnes ; non analysé.

### 4.2 Points chauds par lecture du code et optimisations proposées

Classées par gain estimé (les gains sont des estimations non implémentées) :

1. **Hisser les bruits constants sur la verticale** (E1) : `padded_chunk` de ~10 ms à ~1,5-2 ms pour un chunk rocheux. Gain sur la génération moyenne d'un chunk : ×2 à ×4.
2. **Réduire `column_geo`** (E2) : `ColumnSet::new` de 3,5 ms à ~1,9 ms (sans changer les valeurs), ~0,9 ms avec table de gradients.
3. **Table de gradients pour Perlin** (`noise.rs:57-63`) : 4 appels `cos`/`sin` par octave et par point. Une table de 256 directions (ou le classique hachage → 8 directions) ramène l'octave de 86 ns à ~25-30 ns (estimé), ce qui profite à `height` (0,45 µs), `column_geo`, `rock_block` et au climat. Supprime aussi la dépendance à la libm (E5).
4. **Calculer `column_geo` sur un réseau plus lâche** : le gauchissement (longueur d'onde 800 m) varie lentement ; une interpolation bilinéaire sur un pas de 4 m réduirait ses appels de 16× (la trame de biome, par colonne, reste un hachage). À valider visuellement car elle change les limites de biome au bloc près (`generator_version`).
5. **Évitement des chunks uniformes sous terre** : pour le serveur, un chunk dont tous les blocs sont sous `top − (sol + altération + sédiments)` et au-dessus du Moho est une pure lithologie de socle sans minerai ; générer une liste de segments par colonne (RLE verticale) évite 39 304 appels par chunk. Gain estimé : ×10 sur les chunks souterrains.
6. **Minerais** : `Geology::body_at` (`geology.rs:~290`) boucle sur tous les corps proches pour chaque bloc solide et `Body::ore_at` recalcule `sin_cos(yaw)` à chaque appel (`deposits.rs:368`). Précalculer `sin_cos` dans `Body` ou par `ColumnSet`, et rejeter par boîte englobante en `y` avant. Gain estimé faible à modéré (dépend du nombre de corps proches ; non mesuré car peu de colonnes en ont).
7. **Allocations** : `PaddedChunk` (78 Ko) et `Chunk` (64 Ko) alloués à chaque appel, `Vec<Vec<…>>` de plantes par colonne (1156 `Vec`), `mask` de maillage ; un pool de tampons par fil améliore le passage à l'échelle multi-cœurs (M6).
8. **`river()` n'est pas un point chaud** : appelé 2 à 3 fois par colonne (`height`, `water_top`, `column_geo`) mais à 30 ns ; le mettre en cache par colonne économiserait ~0,1 ms (3 %), sans priorité.
9. **Maillage** (M5) : sortie anticipée pour chunk uniforme ; maillage binaire.

### 4.3 Ce que cela change pour un serveur (estimations, à partir des coûts mesurés)

- Un serveur génère les chunks sans maillage : coût actuel ≈ 3,6 ms/colonne + 4,3 ms/chunk (moyenne `chunkbench`), soit ≈ 11 ms par colonne de 1,7 chunks, ≈ 90 colonnes/s par cœur, ≈ 650 colonnes/s sur 12 cœurs (hypothèse : bonne mise à l'échelle de ×7,4).
- Rayon de vue de 256 m (16×16 colonnes = 256 colonnes) : ~3 s de cœur par joueur entrant ; rayon de 1 024 m (4 096 colonnes) : ~45 s de cœur. Avec les optimisations 1+2 (estimé 3 à 4×), ces temps passent à ~1 s et ~12 s.
- Les chunks souterrains (jusqu'à 91 sous une colonne) ne doivent être générés qu'à la demande : un chunk rocheux coûte aujourd'hui 10-13 ms.
- La génération doit être purement asynchrone et mise en cache par colonne (`ColumnSet` calculé une fois pour tous les chunks de la colonne, voir 4.2).

---

## 5. Architecture des crates (périmètre 3)

**Sens des dépendances : correct.** `core` (serde, jiff, thiserror) ← `worldgen` (core, rand, rand_chacha, blake3, image, rayon, clap, toml, anyhow) ← `voxel` (core, worldgen, clap, image, rayon…) ← `viewer` (tout + Bevy, bevy_egui). Aucune dépendance inverse ; `core` n'a aucun moteur graphique. Bevy et bevy_egui sont figés (`=0.20.1`, `=0.43.1`).

**Taille et cohésion des modules** (lignes) :

| Fichier | Lignes | Remarque |
|---|---|---|
| `worldgen/src/preview.rs` | 1 230 | Rendu PNG des aperçus dans la crate de simulation (dépend de `image`) |
| `viewer/src/fly.rs` | 1 198 | Caméra, flux de chunks, HUD, horloge, UI : à scinder (`streamer`, `hud`, `camera`) |
| `worldgen/src/deposits.rs` | 1 134 | Catalogue, formes, index spatial `BodyIndex`, génération : séparer lecture (serveur) et génération |
| `voxel/src/terrain.rs` | 1 047 | « Objet Dieu » (M9) |
| `worldgen/src/climate.rs` | 952 | Cohérent (une étape) |
| `viewer/src/sky.rs` | 901 | Cohérent |
| `viewer/src/map.rs` | 852 | Cohérent |
| `worldgen/src/geology.rs` | 672 | Cohérent |

**API publique.** `nascio-voxel` expose tous ses modules (`pub mod`), donc des détails internes (`rivers`, `geology::Class`, `terrain::ChunkTint`…) font partie de l'API ; `nascio-worldgen` expose toute la simulation (`pub mod tectonics`, `erosion`…). Pour la phase serveur il faut une façade étroite : `World::load`, `Terrain::from_world`, `ColumnSet`, `generate_chunk`, `DeltaStore`, `Blocks`.

**Ce qui doit migrer ou apparaître pour le serveur :**

1. **`nascio-world` (lecture)** : `world_io`, `grid`, `noise`, `rng`, `biomes` (codes), `blocks` (registre), `deposits::{Body, BodyIndex, DepositCatalog}`, `erosion::upsample::Detail`/`ruggedness`. `voxel` ne dépendrait plus que de cette crate ; la simulation (`tectonics`, `erosion`, `climate`, `geology`, `preview`, `plot`, `progress`, `sun_curves`) resterait dans `worldgen` (et `image` avec elle).
2. **`nascio-protocol`** : types partagés client/serveur (`ChunkPos`, `BlockPos`, `WorldPos`, identifiants de blocs, messages) ; `core` en fournit déjà les coordonnées. Le registre de blocs (noms ↔ `u16`) devra être négocié dans la poignée de main (les identifiants sont attribués par monde).
3. **`nascio-server`** : `Terrain` + `ColumnSet` + `DeltaStore` (réécrit, E3) + file de génération asynchrone + diffusion ; l'horloge (`GameClock`, UTC) vient de `core`.
4. **Hors de `voxel`** : le maillage (`mesh.rs`) et `ChunkTint` ne servent que le client (le serveur n'en a pas besoin) ; à terme `nascio-mesh` ou un module du client. Les outils (`chunkbench`, `topview`, `geology`) vont dans `nascio-tools` (M8).
5. **Le client** ne générera jamais de terrain (invariant 5) : le viewer devra, au passage serveur, recevoir des chunks et non plus appeler `generate_chunk`.

---

## 6. Dette des solutions provisoires (périmètre 4)

Recensée dans `docs/decisions.md` et `docs/roadmap.md` ; risque pour la phase serveur : **H** (élevé), **M** (moyen), **B** (bas).

| Solution provisoire | Source | Risque | Commentaire |
|---|---|---|---|
| `DeltaStore` « god mode » du viewer (copie complète, régions entières, tout chargé) | décisions 2026-11-17 | **H** | Voir E3. |
| Déterminisme flottant inter-machines non exigé | décisions 2026-11-15 | **H** | Voir E5. |
| `[voxel]` hors empreinte et hors version | code | **H** | Voir E4. |
| Rivières statiques, sans écoulement ni animation ; surface arrondie au bloc, paliers sur les fortes pentes ; profondeur 1 bloc (anomalie d) | décisions M6.3 | M | L'eau n'a pas de simulation ; avant le gameplay, décider du modèle (statique, ou automate d'eau) car il conditionne les deltas (eau posée par le joueur). |
| Végétation : un seul sapin, un cactus | décisions M6.4 | M | Le format des arbres (liste de blocs par modèle) est à stabiliser ; chaque ajout change les chunks (`generator_version`). Les plantes dépendent de `column_geo`, donc coûteuses (120 µs par zone de 45 m). |
| Neige saisonnière uniquement en rendu | décisions 2026-11-16 | M | Le serveur et le client verront des blocs différents tant que la neige n'est pas dans les chunks ; seule la surcouche permanente (`snow_layer`) existe. |
| Glace permanente selon le mois le plus chaud interpolé au bloc | décisions M6.2 | M | Source des anomalies (a) et (b) (section 8). |
| Couleurs unies, pas de textures ; teinte d'herbe par sommet | `docs/textures.md` | B | Le client seul. |
| Courants océaniques schématiques, climat par harmoniques sans météo ; nuages du jour fictifs | décisions M4 | B | Compatible avec un serveur déterministe ; la météo viendra plus tard. |
| Altitude tectonique de repli, `Option` pour mondes antérieurs à M3-M5 | décisions 2026-11-15 | M | Code mort à supprimer (M9). |
| Relaxation de talus « provisoire jusqu'à M3 » | décisions 2026-11-15 | B | Je n'en ai plus trouvé de trace dans le code (`grep` : seules des mentions dans `climate.rs`/`main.rs`) ; décision à clore dans le journal. |
| Eau opaque et marquée par l'alpha des sommets (étiquettes 0,4-0,97, 0,995) | décisions 2026-11-16/17 | B | Voir M10, F4. |
| Fond marin : remblai de pente non conservatif (+405 km³) | décisions 2026-11-16 | B | Sans effet sur le jeu. |
| Gisements pétrole/gaz « carte seulement » | décisions 2026-11-16 | B | À faire avec le gameplay. |
| Génération du monde ~140 s (échelle 1), non optimisée | décisions 2026-11-16 | B | Le serveur charge un monde généré ; régénération rare. |
| Horizon sans rivières, sans glace de rivières | spec §5 | B | Client seul. |
| Pas de grottes ni de structures | spec §5 | B | Prévu. |

---

## 7. Rendu : shaders, éclairage, ciel, horizon (périmètre 5)

Fichiers lus : `crates/viewer/src/curvature.wesl` (388 lignes), `curvature_prepass.wesl`, `sky.wesl` (extraits : atmosphère, aurores, soleil), `horizon.rs`, `curvature.rs` (tests). Je n'ai pas lu intégralement `stars.wesl` ni la partie lune de `sky.wesl`. Je n'ai pas profilé le GPU avec un outil externe ; les chiffres GPU viennent du banc du visualiseur (GPU 1,03 ms moyen, 1,43 ms p95).

**Correct ou bien conçu :**
- Courbure d'affichage par `k·d²` dans le vertex shader et dans la passe de pré-calcul/ombres (`curvature_prepass.wesl`, même formule, même origine caméra) : les ombres tombent sur la géométrie abaissée. Rendu relatif à la caméra : les distances `d` sont petites, pas de perte de précision `f32` (`curvature.wesl` : `d = world.xz − curved.drop.yw`). Test de tangence et de marge des boîtes englobantes (`curvature.rs:301-325`).
- Position monde du fragment reconstruite par `wx = curved.sun.x + rel.x` : la précision `f32` à 100 km vaut 7,8 mm, acceptable ; le `repeat` du sampler assure la couture.
- Raccord ciel/relief : fonctions d'atmosphère identiques dans les deux shaders, vérifiées par le test `sky_and_relief_share_one_atmosphere` (`sky.rs:847`).

**Constats :**
- **R1 (moyen) — `local_sun` par fragment** (`curvature.wesl:279-293`) : 2 `asin`, 1 `atan2`, plusieurs `sin`/`cos` par pixel, pour un résultat qui varie très lentement (le soleil change de ~0,0036° par km en x). Le calculer par sommet (les chunks font 32 m) ou le passer en gradient linéaire depuis le CPU supprime l'essentiel du coût ; l'éclairage « par pixel » pour le terminateur peut rester pour les maillages d'horizon (grands triangles). Gain GPU non mesuré ; le GPU n'est pas le goulot sur la RTX 5060 (1,03 ms), mais le sera plus probablement sur la RTX 3050 Mobile.
- **R2 (moyen) — Duplication** : `air_*` et `atmosphere` copiés dans `sky.wesl` et `curvature.wesl` (le test ne garantit que la cohérence de l'exemple testé), `local_sun`/`e_vis_at` dupliqués (`sky.wesl:214` et `curvature.wesl:279`) et dupliqués une troisième fois en Rust dans `nascio-core::astro`. Aucun test n'assure que la version WGSL de `local_sun` correspond à `SunPosition` côté CPU : une dérive serait visible seulement au crépuscule ou près de la couture. Utiliser les imports WESL pour partager le code et ajouter un test qui évalue la fonction WGSL (ou un portage Rust identique) contre `core::astro` sur quelques points, dont la bande de couture.
- **R3 (moyen) — Étiquettes dans l'alpha des sommets** (`curvature.wesl` : `tag < 0.99` eau, `< 0.999` feuillage) et formule d'opacité `(alpha − 0,4) / 0,57` répétée dans le shader : constantes dupliquées avec `mesh.rs` (F4). Le jour où un bloc translucide (verre) s'ajoute, il entrera en collision avec ces étiquettes.
- **R4 (faible)** — `hf` calculé deux fois dans `fragment` ; sans importance.
- **R5 (faible) — Aurores** : boucle de 10 couches par pixel du dôme avec `continue` précoces ; coût limité aux pixels dans l'anneau ; à vérifier sur le portable (non mesuré).
- **R6 (info) — Ombres** : cascades de Bevy calculées pour le soleil de la caméra et appliquées partout (`fetch_directional_shadow`) ; choix documenté (le soleil bouge peu sur la portée des ombres). Près de la couture (bande crépusculaire) les ombres sont masquées par l'éclairage `direct = 0`.
- **R7 (info) — Horizon** : cinq niveaux de 192 cellules (`HorizonConfig::default`), `check_layout` vérifie que le niveau 0 couvre la distance de vue maximale et que l'horizon ne fait pas le tour du tore ; hauteurs tirées du même `Terrain` avec `height_lod` (pas d'aliasing). Le coût CPU de `ground_colors` (texture 512²) est payé au chargement.

---

## 8. Les quatre anomalies de M6 (graine 42, monde d'échelle 5)

Reproduites avec `topview` aux quatre sites (images dans `out/audit/tv/` : `site-0_90496_96896.png`, `site-1_86592_0.png`, `site-2_54592_30016.png`). Les valeurs du monde ci-dessous ont été lues directement dans les couches `out/m6/world-42/layers/*.bin`.

### (a) Limite de banquise rectiligne est-ouest près de (90 900, 97 300)

- **Observé** : sur `site-0`, la mer libre (bleu sombre) s'arrête sur une ligne droite horizontale à z ≈ 97 316 (pixel 420 du site), et la mer gelée (bleu clair uniforme) commence juste au sud, sur toute la largeur de 768 m.
- **Cause (confirmée par calcul)** : la glace de mer est un seuil sur `t_warm` (`Terrain::water_block`, `terrain.rs:531-540`, `water_frozen`, `:524` : gèle si `t_warm < water_ice_warm_c = 0,5 °C`). `t_warm` vient de `SurfaceClimate::column` (`crates/voxel/src/climate.rs`), un **échantillonnage bilinéaire de la grille climat** (cellules de **800 m** dans ce monde, 160 m à l'échelle 1). Près de la couture, le climat ne dépend pratiquement que de z : à x = 90 900, le mois le plus chaud vaut +1,58 °C dans la cellule centrée z = 97 200 et −7,8 °C dans la suivante (z = 98 000), soit une chute de 9,4 °C sur une maille. L'isotherme +0,5 °C de l'interpolation bilinéaire tombe à 97 200 + 800 × (1,58 − 0,5)/9,38 ≈ **97 292 m** (97 368 m si on ajoute le relèvement dû à la hauteur de la cellule terrestre voisine, voir plus bas), ce qui est la ligne observée. Une isotherme d'un champ qui ne varie qu'en z est une droite.
- **Cause aggravante** : `t_warm_sl` ajoute `lapse × hauteur moyenne des terres de la cellule` (`climate.rs`, `lift`). Une cellule de mer adjacente à la terre porte donc un biais chaud, et la mer proche d'une côte montagneuse gèle plus au sud que la mer ouverte.
- **Correctif proposé** : (1) pour l'eau, utiliser la température de la mer (hauteur moyenne 0) et non celle relevée par la terre voisine ; (2) trameer le seuil : `seuil = 0,5 °C + A·bruit(x, z)` avec un bruit périodique de longueur d'onde ~200-400 m et A ≈ 1-2 °C (même principe que `biome_dither_m`), ou une couverture de banquise fractionnée (plaques de glace dont la probabilité croît avec le froid) ; (3) à l'échelle 1 la grille climat fait 160 m : la ligne sera 5× plus nette, il faut donc la trame dans tous les cas. Incrémenter `generator_version`.

### (b) Chenal de rivière gelé et ses berges prolongés sur la banquise, près de (87 000, 400)

- **Observé** : sur `site-1`, une rivière qui descend d'un glacier suit d'abord une diagonale de 45° exactement de 160 px (une maille D8), puis un segment horizontal de ~163 px qui **sort de la côte et s'avance dans la mer gelée** ; on y voit un chenal et deux berges de gravier au lieu d'une glace uniforme.
- **Cause (confirmée par lecture)** : `Rivers::new` (`crates/voxel/src/rivers.rs:184-198`) construit un segment pour **chaque** cellule de rivière dont `flow_direction` pointe vers une cellule voisine, y compris quand cette voisine est la **mer** (cellule sans direction). Le segment va donc du centre de la cellule d'embouchure au **centre de la cellule marine** (jusqu'à une maille au large : 160 m dans le monde d'échelle 5 audité, 32 m à l'échelle 1). Ensuite `Terrain::height` (`terrain.rs:671-677`) applique ce chenal **sans tester si le fond est sous le niveau de l'eau** : le chenal creuse le fond marin à `bed + 0,5` et les berges le **relèvent** à `water_top − 1`, ce qui crée sur la mer gelée un « chemin » de gravier et d'eau libre sous la glace. Le nom « gelé » vient de `water_block` : les deux blocs du dessus de la colonne d'eau d'un chenal sont de la glace.
- **Cause liée** : la diagonale sur la terre montre que les rivières traversent aussi les **glaciers** : `Rivers` ne sait pas que `glacier()` (`terrain.rs:493`) remplace le sol par 12 m de glace. Le chenal est creusé à partir du sommet de la glace, avec de l'eau libre à son sommet gelée sur 2 blocs.
- **Correctif proposé** : (1) arrêter la trace à la côte : pour une cellule d'embouchure dont l'aval est la mer ou un lac, terminer le segment au point d'interpolation où le sol atteint le niveau de l'eau (ou à mi-chemin entre les deux centres), et dans `Terrain::height`/`water_top` ignorer les `RiverHit` quand la hauteur de base est ≤ niveau de l'eau (+ marge de 1 bloc) ; (2) exclure du calcul des cellules de rivière les cellules de calotte/glacier (biome `ice cap / glacier` ou `t_warm` sous `glacier_warm_c`), ou les traiter comme un chenal de glace sans eau libre. Incrémenter `generator_version`.

### (c) Limite nord-sud rectiligne entre roche litée et sol uniforme, près de (55 000, 30 400), vers 2 700 m

- **Observé** : sur `site-2`, à gauche de x ≈ 55 010 on voit des strates de roche sédimentaire nue (couleurs de grès, calcaire, argile) ; à droite, une couleur uniforme (146, 118, 88) qui est **exactement la couleur du bloc `regolith`** (`blocks.toml` : `regolith` color = [146, 118, 88]). Pas de couleur de sol (au-dessus de 2 200 m, `grass_max_elevation_m`, le sol est nul par construction : `terrain.rs:760`).
- **Cause (confirmée par les couches)** : l'épaisseur de régolithe provient de la couche `weathering_depth` (cellules de 800 m dans ce monde), échantillonnée bilinéairement (`geology.rs:200`) puis comparée par `r < weathering.floor()` (`terrain.rs:810-812`) : le régolithe n'existe que là où l'interpolation atteint **1 m**. Sur les lignes de grille voisines, la couche saute de **0,0 m** (cellule i = 68) à **3,98 m** (cellule i = 69) à z ≈ 30 400. L'iso-contour 1,0 m de l'interpolation est donc quasi vertical : sa position x calculée vaut 55 106 (z = 29 600), 55 023 (z = 30 000), **55 011** (z = 30 400), 55 001 (z = 30 800), puis revient vers 54 960 (z = 31 000). La limite observée n'est **pas alignée sur le chunk par construction du code** (rien dans `ColumnSet`/`rock_block` ne dépend de la limite du chunk, ce que j'ai relu) : l'alignement avec x = 55 008 est très probablement une coïncidence (l'iso-contour passe entre 55 001 et 55 022 sur 800 m de long, le bord de chunk 55 008 est au milieu). Je n'ai pas pu exclure formellement un effet de chunk par une expérience contrôlée ; à vérifier en reproduisant avec un décalage de la grille de chunks.
- **Cause racine côté données** : `weathering = regolith_m × (1 − smoothstep(s0, s1, pente))` (`worldgen/src/geology.rs:461-467`) utilise la pente **des moyennes de cellules** (800 m) ; sur un plateau d'altitude un voisin « plat » (4 m) jouxte un voisin « pentu » (0 m). Aucun terme d'altitude ni de gel : 4 m de régolithe à 2 700 m n'a pas de justification physique.
- **Correctif proposé** : (1) dans `rock_block`, moduler l'épaisseur d'altération avec la pente **du bloc** (déjà passée en paramètre) et avec un facteur d'altitude, puis la tramer (`floor(w + hachage(x, z))`) pour que la limite soit un bord irrégulier et non une droite ; (2) dans la génération, lisser `weathering_depth` (l'écart de 0 à 4 m entre deux mailles est un artefact de la pente en cellules). Incrémenter `generator_version`.

### (d) Rivières d'au plus 2 blocs de profondeur partout

- **Observé** : une colonne d'eau de 1 bloc dans un chenal de gravier ; jamais plus.
- **Cause (confirmée par les couches, aux deux échelles)** : la profondeur physique est `river_depth = 0,3 · Q^0,4` (`erosion/mod.rs:186-187`, `:447`). Dans le monde d'échelle 1 (`out/pre-m6/world-42`), le débit maximal du monde est **Q = 5,96 m³/s** : profondeur maximale **0,61 m**, 99,9 % des cellules de rivière sous 0,56 m, largeur maximale 6,1 m (monde d'échelle 5 : Q = 6,86 m³/s, 0,65 m, 6,5 m). Dans `Rivers::at` (`rivers.rs:274`), `blocks = round(profondeur × (1 − frac²)).max(1)` vaut donc toujours 1 : aucune cellule n'atteint 1,5 m (seuil de 2 blocs). Ce n'est pas un bogue de code mais le résultat de la géométrie hydraulique appliquée à un monde de 10 000 km² dont le plus gros bassin draine ~300 km² (Q·année / précipitation ≈ 5,96 × 3,15·10⁷ / 0,6 m).
- **Correctif proposé (décision de l'auteur)** : le monde est une version réduite de la Terre : les rivières physiques sont petites. Pour du gameplay (barques, ponts, pêche), recalibrer `river_depth_coef` (par exemple ×4 à ×8) et `river_width_coef`, ou appliquer une compression de rang (profondeur ∝ ordre de Strahler : 1 bloc pour les ruisseaux, jusqu'à 4-6 blocs pour le plus grand fleuve). Compléter par `blocks = ceil(...)` plutôt que `round` et un minimum de lit plus profond en bas de pente. Incrémenter `generator_version`. Garder la formule physique en option de configuration.

#### Ajout du 17 novembre 2026 : constat de l'auteur dans le visualiseur

*Ajouté par Claude après l'audit. L'audit ne traitait que la profondeur.*

L'auteur juge les rivières problématiques sur trois points :
- **Trop fines.**
- **Tracé en longs segments droits.** Les rivières devraient se courber comme des rivières naturelles, avec des méandres en S réguliers comme ceux que forme l'érosion.
- **Profondeur incohérente** à de nombreux endroits : le chenal creuse des sortes de canyons.

**Objectif.** Les rivières n'ont pas à être réalistes du point de vue du débit. Elles doivent :
- avoir l'air réalistes ;
- présenter des défis de jeu : être assez larges et profondes pour qu'il faille construire des ponts, et être utilisables par l'industrie.

**Pistes, issues de la lecture de ce rapport et non vérifiées :**
- **Segments droits.** Le tracé suit `flow_direction` (D8) d'un centre de cellule au suivant, avec seulement deux passes de lissage (`river_smooth_passes`). Sur le monde d'échelle 5 utilisé pour l'audit et les vues de M6, la maille vaut 160 m, ce qui donne de longues droites et des diagonales à 45°. Il faut ajouter des méandres procéduraux : un déplacement latéral périodique dont l'amplitude et la longueur d'onde croissent avec la largeur (longueur d'onde d'environ 10 à 14 fois la largeur), puis interpoler le tracé avec une spline.
- **Canyons.** Le chenal est creusé relativement au niveau d'eau, qui est monotone vers l'aval et arrondi au bloc. Le relief local peut donc rester très au-dessus de l'eau, sur le lit comme sur les berges. Il faut une vallée dont les talus sont bornés en pente de part et d'autre du chenal, au lieu d'une simple coupe verticale.
- **Largeur et profondeur.** Il faut une compression de rang, avec une largeur et une profondeur de gameplay qui suivent l'ordre de Strahler, comme proposé plus haut.

Ces trois points sont à regrouper avec (b) et (d) dans une **refonte des rivières**, et un seul changement de `generator_version`.

---

## 9. Ce qu'il reste à mesurer sur le portable Linux (RTX 3050 Mobile)

Aucune mesure n'a été faite sur ce portable : toutes les valeurs de ce rapport viennent de la machine Windows. À mesurer, en utilisant un monde d'échelle 1 régénéré (`cargo run -p nascio-worldgen --release -- generate --seed 42 --scale 1 --out out/m2/world-42`) :

1. `chunkbench` sur 1 cœur et tous les cœurs (colonnes/s, ms par colonne, ms par chunk en génération et en maillage) ; nombre de cœurs et type (P/E) du portable ; comparaison avec la référence de 65 / 524 colonnes/s.
2. Banc du visualiseur : `nascio-viewer <monde> --bench bench/default.toml --bench-out out/bench.csv` : moyenne, p95, p99, 1 % low, nombre d'images au-delà de 16,7 ms (critère de 60 i/s), part CPU et GPU, à 1920×1080 et à la résolution native de l'écran, avec et sans ombres (`--no-shadows`), avec la distance de vue de 1 024 m et de 512 m.
3. Coût GPU des shaders : temps par passe (le banc l'enregistre déjà), en particulier l'éclairage par fragment (`local_sun`), l'atmosphère et la boucle d'aurores dans les zones de couture ; comparer avec 1,03 ms moyen mesuré sur la RTX 5060.
4. Mémoire : pic de RAM et de VRAM pendant le vol rapide de la trajectoire de banc ; temps de chargement (`bench: loaded in …`, 10,9 s sur la RTX 5060).
5. Déterminisme inter-plateformes : générer le monde d'échelle 25 (graine 42) sur Linux et comparer l'empreinte (`World::hash`) à celle de Windows ; comparer le hachage de ~20 chunks fixes (coutures comprises). C'est la mesure la plus importante pour décider de la question E5.
6. Chargement de l'ensemble du monde d'échelle 1 (`Terrain::from_world`, 0,94 s en échelle 1 et 43 ms en échelle 5 sur Windows) et empreinte du monde (`World::hash`, blake3 sur ~370 Mo).
7. Débit de génération sans maillage (serveur simulé) sur 1 à N cœurs, à rayon de vue de 256 m et 1 024 m.

---

## 10. Plan d'action proposé avant la phase « Serveur et réseau »

Ordre recommandé (chaque étape se valide par les tests existants et un hachage de chunks avant/après quand la sortie ne doit pas changer) :

1. **Figer la base de mesure.** Committer ou écarter les modifications de l'étape 7 (`topview`, `Cargo.toml`). Régénérer un monde d'échelle 1 à la version courante. Ajouter les hachages de référence (monde d'échelle 25 et 20 chunks fixes) en test, exécutés sous Linux (CI) et Windows. [E5, E6, M11, F7, F8]
2. **Versionner la configuration de génération.** Mettre `[voxel]` dans `world.toml`, l'inclure dans `World::hash` ou dans un hachage de génération du serveur, et refuser une autre configuration côté serveur. [E4]
3. **Optimisations sans changement de sortie.** Hisser les bruits constants en `y` (`ColumnSet`), supprimer le triple calcul du gauchissement et le biome perdu, précalculer `sin_cos` des corps de minerai, pool de tampons. Valider par égalité bit à bit des chunks (hachage). Mesurer avec `chunkbench`. [E1, E2, M6]
4. **Corrections des anomalies** (changent la sortie : une seule hausse de `generator_version`) : embouchures de rivières et glaciers (b), seuil de glace de mer tramé et température de mer sans relèvement terrestre (a), altération au bloc et tramée (c), refonte des rivières (calibrage (d), méandres, vallées sans canyons, largeur jouable ; constat de l'auteur, voir l'ajout au §8). Revérifier avec `topview` aux quatre sites. [M1-M4]
5. **Table de gradients pour le bruit** (change les valeurs ; à grouper avec l'étape 4 pour un seul changement de version) : supprime la dépendance à la libm. [E2, E5]
6. **Scinder les crates.** Extraire `nascio-world` (lecture) de `worldgen`, sortir les outils de `voxel` dans `nascio-tools`, retirer les `Option` des mondes anciens de `Terrain`, déplacer `chunk_delta` dans `core::torus`. [M7-M9, F1]
7. **Réécrire le magasin de deltas** pour le serveur : régions indépendantes verrouillées, chargement paresseux, journal append-only avec compactage, instantanés par chunk. Garder le format de fichier. Tests de propriétés à travers la couture. [E3, M11]
8. **Préparer le protocole** : chunk uniforme et palette/RLE, négociation du registre de blocs, version du générateur dans la poignée de main. [F5]
9. **Rendu (peut se faire en parallèle, côté client)** : `local_sun` par sommet, partage du code WGSL, test CPU/GPU de `local_sun`, attribut de sommet pour les étiquettes d'eau et de feuillage, maillage binaire. Mesurer sur le portable avant et après. [M5, M10, R1-R3]
10. **Mettre à jour les documents** : `docs/decisions.md` (normales de la courbure, relaxation de talus, anomalies closes), `docs/roadmap.md` (références de banc à l'échelle 1), `docs/world-spec.md` (hydraulique des rivières, glace de mer).

---

## 11. Réserves

- Les gains d'optimisation indiqués sont des **estimations** à partir des coûts mesurés ; aucune des optimisations n'a été implémentée ni testée.
- Je n'ai pas lu intégralement `stars.wesl`, la partie lune de `sky.wesl`, `map.rs`, `perf.rs`, `bench.rs`, ni les étapes de simulation de `worldgen` (tectonique, érosion, climat) au-delà de ce que le déterminisme et les anomalies exigeaient ; les tests de ces étapes passent.
- L'alignement de l'anomalie (c) avec le bord de chunk est expliqué comme une coïncidence probable, non démontrée de façon contrôlée.
- Les chiffres GPU et de latence viennent d'une seule exécution du banc sur la RTX 5060 ; l'exécution de référence de M6 avait donné 3,72 ms de moyenne (la différence de 4 % n'est pas significative pour une machine non isolée).

