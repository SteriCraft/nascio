# Journal des décisions

Décisions en vigueur, sous leur forme actuelle. Une décision remplacée est réécrite ou retirée (l'historique est dans git et `docs/reports/`). Les valeurs réglables vivent dans la configuration TOML ; ce journal en donne le sens.

## Projet et outillage

| Date | Décision | Raison |
|---|---|---|
| 2026-11-15 | Rust de bout en bout ; Bevy + egui pour le visualiseur et la fenêtre du serveur | Performance, un seul langage, compilateur strict |
| 2026-11-15 | Code en anglais, documentation en français | Convention de l'auteur |
| 2026-11-15 | Versions des dépendances figées (`=x.y.z`, liste dans `Cargo.toml`) ; édition 2024 ; toute mise à jour est une décision explicite | Reproductibilité |
| 2026-11-15 | `bevy/dynamic_linking` derrière la fonctionnalité `dev` du visualiseur (`cargo viewer-dev`) | Compilation rapide sans toucher aux builds release ni à la CI |
| 2026-11-17 | CI sur Linux et `windows-latest` ; hachages de référence (monde 1/25, 24 chunks dont les coutures, bits des hauteurs sur 64 × 64 points) liés à `GENERATOR_VERSION` | Détecter toute divergence entre plateformes |
| 2026-11-15 | Objectif de performance : p95 ≤ 16,7 ms en 1080p, réglages maximum, sur le portable de référence (Ryzen 5 5600H, RTX 3050 Mobile, Linux) ; banc scripté `bench/default.toml` (CSV par image, résumé p95/p99/1 % low) | Décision de l'auteur |
| 2026-11-17 | Outils de développement dans `nascio-tools` (`chunkbench`, `topview`, `geology`, `netbench`, `site`, coupes) ; `voxel` sans `clap` ni `image` | Crates de génération légères |
| 2026-11-17 | `--dev-override` : seule façon d'utiliser une autre config `[voxel]` ; toute sortie produite avec est marquée `DEV-OVERRIDE` (titre, noms de fichiers, première ligne), deltas en mémoire seulement | Aucune sortie ne doit passer pour une sortie du générateur |
| 2026-11-17 | `--screenshot` capture en 1920×1080 et ignore clavier et souris ; `--clouds` force la couverture nuageuse | Captures comparables |
| 2026-11-19 | Limites connues dans `docs/limites.md` (L1, L2…, numéros jamais réutilisés) ; travaux restants en cases à cocher dans « Reste à faire » de `roadmap.md` et `roadmap-serveur.md` | Retrouver le travail restant au moment de choisir |
| 2026-11-17 | Liste des textures à faire : `docs/textures.md`, complétée à chaque nouveau bloc | Suivi des blocs sans texture |

## Monde, temps et astronomie

| Date | Décision | Raison |
|---|---|---|
| 2026-11-15 | Monde torique de 100 km × 100 km, voxels de 1 m | Monde fini sans bord ni projection impossible |
| 2026-11-15 | Latitude `φ = asin(d)`, couture = ligne des pôles | Surfaces par latitude identiques à une sphère |
| 2026-11-15 | Saisons atténuées à zéro près de la couture (`δ_eff`) | Soleil et climat continus à la couture |
| 2026-11-15 | Bande crépusculaire à la couture : soleil sous l'horizon, aurores en anneau au bord (visibles de loin), pas d'ombres ; couvert nuageux permanent (opaque jusqu'à `S_LIGHT`, dégagé à `S_CLEAR`), climatique et pas seulement visuel | Cacher le basculement inévitable du soleil et de la voûte céleste |
| 2026-11-15 | Fuseaux horaires : heure solaire selon la longitude | Réalisme, jour et nuit sur la carte |
| 2026-11-15 | 1 jour = 1 h réelle, 365 jours par an ; temps en secondes Unix depuis `GAME_EPOCH` (2026-01-01T00:00:00Z), `t < 0` valide | Déterminisme ; le site calcule l'heure seul |
| 2026-11-15 | Obliquité fixe de 23,44° | Saisons terrestres |
| 2026-11-15 | Azimut depuis le nord (−z) vers l'est (+x), dans [0, 2π) ; soleil = (cos e·sin A, sin e, −cos e·cos A) | Convention géographique, z vers le sud |
| 2026-11-15 | `torus::delta(a, b)` vaut +S/2 à égalité exacte dans les deux sens | Conforme à la spec §1.2, testé |
| 2026-11-15 | Lune : lunaison de 29,53 jours de jeu, nœuds fixes, phase à l'époque `moon_phase_at_epoch_deg` ; étoiles : catalogue réel, 366 rotations sidérales par an | Ciel nocturne réaliste et simple |
| 2026-11-16 | Lumière : directe de 0° à +10° de `e_vis`, du ciel de −6° à +10° ; aucune lumière directe ni ombre dans la bande | Fondu doux qui garde l'invariant de la bande |
| 2026-11-16 | Pression : atmosphère standard 1976 comprimée ×2 (altitude `h` lit la Terre à `2h`), nulle à 50 km (`nascio-core::atmosphere`, `[atmosphere]`) ; gradient thermique −6,5 °C/km (non doublé), plancher −56,5 °C | Montagnes de 3 km sans gel général |

## Génération : règles générales

| Date | Décision | Raison |
|---|---|---|
| 2026-11-15 | Seul le serveur génère ; on stocke la carte macro et les modifications des joueurs | 10¹⁰ colonnes ; pas de déterminisme flottant entre machines requis |
| 2026-11-15 | Chunks cubiques de 32³ ; grilles tectonique de 160 m et fine de 32 m ; résolutions réduites 1/5 et 1/25 | 100 000 = 32 × 3 125 ; grilles alignées sur les chunks |
| 2026-11-15 | Format `.bin` : en-tête `NSCL`, version 1, type (u8/u16/u32/f32), dimensions, cellule (f64), unité, données little-endian | Simple, typé, autodescriptif (§6) |
| 2026-11-15 | Parallélisme `rayon` en *gather* depuis l'état précédent, égalités tranchées par des règles fixes | Résultat indépendant du nombre de fils |
| 2026-11-17 | Mathématiques de la génération (`core`, `worldgen`, `voxel`, horizon, physique) sur `libm` via `nascio_core::fmath::DetMath`, imposé par clippy `disallowed-methods` ; rendu et outils exemptés | La libm du système diffère entre Windows et Linux (constaté) ; `libm` donne les mêmes bits partout |
| 2026-11-17 | Bruit de Perlin : table de 256 gradients à angles réguliers | Rattrape le coût de `libm` |
| 2026-11-17 | Empreinte de génération = blake3(`World::hash`, hachage de `[voxel]`) ; `[voxel]` = `VoxelConfig::default()` (non stockée), liée à `GENERATOR_VERSION` par un test ; tout changement de génération ou de `[voxel]` incrémente `generator_version` (actuelle : **20**) | La version couvre le code et la config ; les deltas sont rattachés à l'empreinte |
| 2026-11-18 | Monde de référence : `out/server/world-42` (graine 42, échelle 1, v20), données `out/server/data` | Mesures et essais comparables |
| 2026-11-16 | Progression de la génération (fenêtre `--generate`, ligne de console) lue sur des compteurs | Sans effet sur le déterminisme |

## Tectonique et relief

| Date | Décision | Raison |
|---|---|---|
| 2026-11-15 | Plaques déplacées par sauts entiers de cellules depuis une translation cumulée en f64 ; pas de rotation | Pas de déplacement sous-cellule perdu |
| 2026-11-15 | Soudage des plaques après `collision_shortening_m` (5 km) de convergence : vitesse commune pondérée | Sans forces, les collisions détruisaient la croûte continentale |
| 2026-11-15 | Filtre majoritaire sur les ids de plaque à chaque pas | Frontières propres |
| 2026-11-15 | Fraction continentale finale 0,35 (±0,02), tectonique relancée au plus 4 fois en corrigeant la fraction initiale | Environ 29 % de terres émergées |
| 2026-11-15 | Éléments étroits d'au moins `min_feature_cells` = 1,5 cellule | Visibles en 1/25 |
| 2026-11-15 | Arcs insulaires = crête immergée (−800 m) ponctuée d'îles selon un bruit ; points chauds actifs par fraction de déplacement (`hotspot_duty`), édifices de 6 000 m au-dessus du plancher, perte sur 120 Ma | Îles espacées régulièrement, pas d'anneaux de terre |
| 2026-11-15 | Rifts en graben continu (250 m, 1 200 m de large) ; petites inclusions océaniques (< 50 km²) en eaux intérieures | Lacs façon Écosse (décision de l'auteur) |
| 2026-11-15 | `moho_depth` = profondeur du Moho sous le niveau de la mer ; `crust_thickness` = surface → Moho ; profondeurs de croûte ≈ 1/10 de la Terre, relief jusqu'à ≈ 3 000 m ; Moho conservé sans racine sous les chaînes | Mines praticables, reliefs crédibles |
| 2026-11-15 | Terres calées par rang sur l'hypsométrie terrestre ramenée (`[tectonics.hypsometry]`) ; fond océanique non calé (`calibrate_deep = false`) : dorsale −900 m, abysse −1 800 m, fosses −2 500 m | Ordre physique conservé ; océan moins profond (demande de l'auteur) |
| 2026-11-16 | Marges : plateau sur `shelf_share` = 20 % de la croûte continentale (≤ 5 km), talus de 5 km sur toutes les marges ; pente du fond ≤ 0,35, pente côtière des terres ≤ 0,8 × distance à la mer (`seafloor`), sur les deux grilles | Pas de murs vers la mer ; sommets vers 3 000 m |
| 2026-11-16 | Côtes : distorsion de domaine de 0,8 cellule (`coast_warp_cells`) au suréchantillonnage de l'altitude tectonique | Pas de côtes en escalier de 160 m |

## Érosion et hydrologie

| Date | Décision | Raison |
|---|---|---|
| 2026-11-15 | Grille fine = suréchantillonnage bicubique + bruit de détail périodique selon la rugosité ; les chunks ajoutent les octaves fines (`erosion::upsample::Detail`) | Même relief à toutes les résolutions |
| 2026-11-15 | Érosion : 20 pas de 50 ka ; priority-flood, D8, stream power implicite (n = 1, m = 0,45, K = 3·10⁻⁶), diffusion de Roering (D = 0,01) ; mer = niveau de base | Calage graine 7 |
| 2026-11-16 | Soulèvement pendant l'érosion : `uplift_max_m_yr` (0,5 mm/an) × convergence actuelle normalisée, sur les orogènes ; sutures soudées à 0,5 de la convergence initiale ; couche `uplift_rate` | Sans lui, l'érosion n'abaisse que les reliefs |
| 2026-11-16 | Sédiments côtiers en cônes de dépôt sous `−1 m − 2 % × distance` (`deposit_shelf_slope`), au lieu d'une diffusion marine | Masse conservée, coût raisonnable |
| 2026-11-15 | Mer = régions sous 0 m d'au moins 50 km² ; lacs = dépressions ≥ 2 m et ≥ 20 000 m², les autres comblées ; rivières ≥ 0,1 m³/s, Strahler | Plan M3 |
| 2026-11-16 | Hydrologie finale avec la précipitation du climat ; largeur `2,5·Q^0,5`, profondeur `0,3·Q^0,4` (couches `river_width`, `river_depth`) ; pas de deltas | Les cônes de dépôt tiennent lieu de deltas |
| 2026-11-17 | Rivières des chunks : gabarit de jeu par Strahler sur tout le drainage (eau dès l'ordre 4, 3 à 40 m de large, 1 à 5 m de profondeur) ; formule physique par `river_gauge = "physical"` | Rivières jouables (ponts, industrie) |
| 2026-11-17 | Tracé des rivières : Catmull-Rom et méandres « sine-generated » (12 largeurs, ≤ 3 largeurs), effacés de 1,5 % à 8 % de pente moyenne ; berges au niveau de l'eau, vallée ≤ 0,6 ; arrêt à la mer ou au lac ; pas d'eau sous la glace permanente ; paliers de surface acceptés | Pas de segments droits ni de canyons |

## Climat et biomes

| Date | Décision | Raison |
|---|---|---|
| 2026-11-15 | Climat stocké en harmoniques annuelles (`_x0`, `_a1`, `_p1`, `_a2`, `_p2`, phases en jours) | Transitions continues, données compactes |
| 2026-11-16 | Température : profil calé sur l'insolation annuelle relative (`mean_profile`) + part saisonnière ; inertie exacte harmonique par harmonique (retard 30 j terre, 52 j mer) ; saison marine amortie (`sea_amplitude` 0,4) ; influence maritime `exp(−d / 20 km)` sur la température | Tropiques plats, fort gradient 30–70°, côtes douces |
| 2026-11-17 | Couture : plancher de la mer (−1,8 °C) effacé sur la banquise pérenne (`perennial_ice_fade_c` 5 °C) ; moyenne zonale ≈ −19 °C, testée contre la cible §4.3 | Sur une banquise, la surface est de la glace |
| 2026-11-16 | Vents à trois cellules par hémisphère, ZCIT décalée de `0,6·δ_eff`, calme à la couture ; courants schématiques constants (chaud à l'ouest des bassins, remontée froide à l'est) | Spec §4.3 |
| 2026-11-16 | Pluie par saturation (Magnus, pression comprimée) le long du vent, advection relative à la référence marine de la latitude ; condensation en mer = brouillard ; bande polaire `max(P, 1 600 mm·c_min)` | Pas de traînées de pluie sur les petits reliefs |
| 2026-11-16 | Sorties : `snow_fraction` = part du sol enneigée (neige aussi calculée en mer), `cloud_cover`, `humidity` (g/kg), `diurnal_range` | Le rendu et le HUD en ont besoin |
| 2026-11-16 | Biomes Köppen simplifiés sur la grille fine (14 codes, seuils `[biomes]`), température corrigée de l'altitude fine | Glaciers de sommet |
| 2026-11-17 | Transitions de biomes : lecture à une position tramée (±64 m) après une déformation de 250 m | Frange mêlée d'environ 100 m |

## Géologie et gisements

| Date | Décision | Raison |
|---|---|---|
| 2026-11-16 | Colonne géologique jusqu'au Moho (lissé par trois flous de 2 cellules), puis `magma` solide et mortel à l'infini ; croûte ≥ 50 m | Limite physique du monde vers le bas |
| 2026-11-16 | Couverture sédimentaire jusqu'à ≈ 500 m ; l'orogène passe avant l'arc comme contexte dominant | Bassins réduits comme la croûte ; schistes et gneiss aux sommets |
| 2026-11-16 | Géotherme exponentiel (`k` = 2,7, `[geotherm]`) : ≈ 50 °C à 500 m ; danger de la roche chaude laissé au gameplay (HUD au-delà de 60 °C) | Miner sans danger les premières centaines de mètres |
| 2026-11-17 | Altération amincie par la pente et l'altitude (2 200 à 3 200 m), arrondi tramé ; régolithe tenu sur pentes fortes sous climat tropical | Pas de limites droites de régolithe |
| 2026-11-16 | Catalogue `config/deposits.toml` copié dans le monde (dans l'empreinte) : blocs par nom, contextes et poids, contextes requis (`"a\|b"`), formes (`vein`, `stock`, `lens`, `layer`, `blanket`, `placer`, `trap`), profondeur, taille, teneur, rareté, raffinage ; ids attribués à la génération | Types modifiables sans toucher au code |
| 2026-11-16 | Corps tirés par tuile de 2 km (RNG par tuile) ; ≈ 0,1 % de minerai dans les 500 premiers mètres sous les continents, or ≈ 15 fois plus rare que le fer | À revoir avec le gameplay |
| 2026-11-17 | Contextes `passive_margin`, `tropical`, `land` (terre ferme) ; `max_temperature_c` par type (pétrole 120 °C, gaz 200 °C) ; aucun type exclusivement au-delà de 60 °C | Pétrole en mer crédible, gisements atteignables |
| 2026-11-18 | Champs `host_rock` (s'ajoute au filtre de forme), `above` + `probability` (corps posé sur un parent, rangé en tête pour minéraliser en premier), `replaces_soil` (manteaux, sauf le bloc de surface) | Silex dans la craie, chapeau de cuivre oxydé, fer des marais affleurant |
| 2026-11-17 | Pétrole et gaz en carte seulement (couche `traps` en bits : province et piège par type) | Pas de bloc avant le gameplay |
| 2026-11-18 | Uranium des orogènes approximé : filons dans tout le contexte `orogen` | Les plutons ne sont pas un contexte |

## Blocs et chunks

| Date | Décision | Raison |
|---|---|---|
| 2026-11-16 | Registre de blocs par monde (`blocks.toml`) : blocs intégrés (`BUILTIN`, nouveaux blocs ajoutés **en fin**) puis blocs du catalogue ; blocs désignés par leur nom ; un nom absent d'un ancien monde retombe sur un bloc proche | Indices stables, robustesse aux changements |
| 2026-11-15 | Génération par colonne de chunks avec bordure d'un bloc, fonction pure ; chunks uniformes détectés sans remplissage ; `Chunk` uniforme sans tableau | Voisins toujours concordants, couture comprise |
| 2026-11-17 | Surface des chunks et de l'horizon : `sample_bicubic_monotone` (Hermite, tangentes Fritsch–Carlson) | Pas de dépassement ni de gradin aux lignes de nœuds |
| 2026-11-15 | Sable seulement au niveau de l'eau et dessous | Pas de plaines littorales ensablées |
| 2026-11-17 | Sols par biome (`[[voxel.soils]]`), épaisseur réduite avec la pente ; herbe unique, teinte en dégradé selon le climat (par sommet) | Transitions en dégradé, même avec les textures |
| 2026-11-17 | Glace et neige permanentes selon le mois le plus chaud ramené au bloc ; `snow_layer` (1/8 de bloc) en frange ; banquise de 2 m, seuil tramé (±1,5 °C) | Glaciers et banquise sans lignes droites |
| 2026-11-17 | Végétation provisoire : sapin et cactus, une plante au plus par cellule de 5 m par hachage, densités par biome (`[[voxel.vegetation]]`), limite des arbres à 10 °C, pente ≤ 0,7 | En attendant l'étape « Végétation » |
| 2026-11-17 | Maillage glouton par masques de bits avec occlusion ambiante par sommet ; eau en maillage séparé contre l'air ; canal matériau en UV0 (`MeshData::material`) | 0,27 ms par chunk ; alpha réservé à l'opacité |
| 2026-11-18 | Crate `nascio-chunk` (dépend de `core` seulement) : table des blocs, `Chunk`, `ChunkTint`, maillage, types de tuiles d'horizon | Le client ne peut pas générer (vérifié par `no_generation.rs`) |

## Deltas et stockage

| Date | Décision | Raison |
|---|---|---|
| 2026-11-17 | Deltas en valeur absolue, régions de 25 × 25 colonnes, en-tête avec version, empreinte et noms des blocs ; autre monde refusé, autre version chargée avec avertissement | Robuste aux changements du générateur ; 25 divise 3 125 |
| 2026-11-18 | Journal global `deltas/wal/w.<lsn>.log` (CRC-32C, numéros), points de reprise tous les 16 Mio qui réécrivent les régions (format 2, LSN) ; `fsync` groupé toutes les 50 ms (`fsync_interval_ms`) ; écritures durables (temporaire, `fsync`, renommage) | Perte d'un tick au plus après une coupure, sans plafonner le débit |
| 2026-11-18 | Enregistrement invalide en fin du dernier segment = écriture interrompue, retirée ; ailleurs = corruption, ouverture refusée | Ne pas masquer un disque abîmé |
| 2026-11-17 | Régions chargées à la première lecture, bornées en mémoire ; instantanés par chunk ; `ColumnCache` borné ; réglages dans `DeltaConfig`, hors empreinte | Une édition en 0,007 ms |

## Serveur et réseau

| Date | Décision | Raison |
|---|---|---|
| 2026-11-18 | Hébergement : un serveur Linux dédié | Décision de l'auteur |
| 2026-11-18 | Un processus : réseau `tokio`/`quinn`, simulation sur son propre fil à 20 Hz (ordre des identifiants), fil d'écriture du journal ; la simulation ne touche jamais une connexion (canaux par client) ; mouvement des joueurs sur le fil de simulation, sans parallélisme | Tick prévisible ; 460 joueurs mesurés sous 3 ms |
| 2026-11-18 | TLS `rustls` + `ring`, certificat auto-signé (`rcgen`) épinglé à la première connexion (`known_servers.toml`) | Pas de CMake/NASM sous Windows |
| 2026-11-18 | Socket UDP par `socket2` avec 4 Mio de tampons (`udp_buffer_bytes`) | Poignées de main perdues dans les rafales |
| 2026-11-18 | Protocole `nascio-protocol` sans réseau : contrôle en `postcard`, chunks et entités encodés à la main ; `PROTOCOL_VERSION` incrémentée à chaque changement (actuelle : 12) | Partagé par serveur, client et bots |
| 2026-11-18 | Chunks : codec v2 (plages varint) + zstd niveau 3 par colonne, fait une fois en cache ; un flux QUIC unidirectionnel par colonne (16 en vol), octet `UniKind` en tête ; au-dessus d'une colonne, de l'air ; sous sa plage, chaque colonne locale répète son bloc le plus bas | ≈ 1,3 Ko par colonne terrestre ; pas de blocage en tête de file |
| 2026-11-18 | Version d'une colonne = LSN de sa région ; abonnement avant lecture ; lots `Blocks` appliqués aux colonnes plus anciennes ; `ClientMsg::Resend` pour une colonne éditée sous la plage du client | Aucun changement perdu entre lecture et abonnement |
| 2026-11-19 | Éditions prédites par le client : surcouche par cellule (file des éditions en attente) au-dessus du miroir, lue par la physique, le maillage et la visée ; un `Blocks` plus récent que la colonne confirme l'édition la plus ancienne s'il porte le même bloc, sinon vide la file (le serveur a raison) ; `EditRefused` retire la plus ancienne ; sans réponse après `edit_timeout_ms` (1 000, `client-settings.toml`), l'édition tombe ; ni l'inventaire ni une pose dans la boîte du joueur ne sont prédits ; protocole inchangé | L'édition s'affiche dans l'image même du clic |
| 2026-11-19 | Remaillage ciblé : une cellule éditée remaille sa colonne, plus la voisine en x et/ou en z si elle est sur ce bord (la diagonale pour un coin, à cause de l'occlusion ambiante) ; une confirmation conforme ne remaille rien ; ces remaillages passent avant les colonnes en file (file prioritaire des tâches du `Streamer`) ; pas de remaillage par chunk (0,37 ms par chunk, 1 à 3 ms par colonne : gain trop faible, L18) | Au lieu de 9 colonnes entières derrière les colonnes neuves |
| 2026-11-18 | Une colonne porte ses teintes (`ChunkTint`, `f32` exacts) calculées par le serveur | Maillage du client identique bit à bit, sans climat local |
| 2026-11-19 | Budget par client 512 Ko/s (seau à jetons) : état du joueur, entités, éditions, colonnes, puis tuiles | À 128 Ko/s et ≈ 3 Ko par colonne (végétation), la zone reçue plafonnait vers 300 m en mouvement ; un plafond, pas une consommation (immobile, presque rien) ; réglable par l'hébergeur |
| 2026-11-18 | Entités : positions au 1/64 m, différences toriques, base par entité acquittée (`seq`, ≤ 64) ; priorité par accumulateur (64 par tick + toute entité vieille de 4 ticks, rayon 256 m) ; réplication parallèle par client (`replication_threads` 4) ; affichage 100 ms en retard, extrapolation ≤ 250 ms | 5 Hz garantis, 20 Hz au plus près, déterministe |
| 2026-11-18 | Invariant 5 : horizon en tuiles du serveur (grille fixe de pas 16·2^L m, 32×32 sommets : hauteur, eau, nature, couleur), ciel par `WorldSky` (climat réduit 125², sol lointain 512², ni graine ni géotherme) ; client sans monde local, cache `client-cache/g<version>-<empreinte>/` | Le client ne peut rien générer |
| 2026-11-18 | Découverte UDP sur `discovery_port` 7778 (réponse au plus aussi grande que la sonde de 1 200 o) | Connexion par adresse seule, sans amplification |
| 2026-11-18 | Chat (protocole) : le serveur envoie des faits, le client les formule ; 256 caractères, 1/s rafale 5 ; commandes `message`, `tell` | Langue du client |
| 2026-11-18 | Limites par client : contrôle 200/s (rafale 400), éditions 20/s (rafale 40), commandes 40 datagrammes/s ; expulsion après 500 refus ou un dépassement de plus de 10 s (`overrun_kick_after_s`) ; blocage d'une adresse après 5 échecs de connexion par minute ; `abuse.log` échantillonné ; contrôles sur le fil de lecture | Un abus ne coûte rien au tick ; le Wi-Fi instable n'est pas expulsé |
| 2026-11-18 | Au plus `account_hash_concurrency` hachages Argon2id à la fois (moitié des cœurs) | Le tick garde ses cœurs |
| 2026-11-18 | Refus de démarrer avec `server_id = "dev"` hors adresse locale ; `bind` par défaut `127.0.0.1:7777` | Un serveur public a sa propre audience |

## Comptes et administration

| Date | Décision | Raison |
|---|---|---|
| 2026-11-18 | Comptes nom + secret en Argon2id (19 Mio, 2 passes, par compte, sel 16 o) dans `accounts.toml` (écriture durable) | Standard actuel ; un fichier suffit pour 2 000 comptes |
| 2026-11-18 | Joueurs connectés par jeton du site : `base64url(postcard(Claims) ‖ Ed25519)` strict, 5 min, usage unique, émis après démarrage + 30 s, nonce en mémoire ; format et vecteurs dans la spec §8 ; un jeton valide crée le compte | Inscription et invitation sur le site |
| 2026-11-18 | `secret_login` = `"admin"` par défaut (`"all"`, `"none"`) ; `open_registration` faux, `--open-registration` pour le développement | Le secret reste la porte des administrateurs |
| 2026-11-18 | Bannissement par compte (temporaire ou permanent) dans `accounts.toml`, vérifié après le secret ; console `ban`, `unban`, `bans`, `admin`, `kick`, `fly` (met aussi en mode dieu), `god`, `spawns`, `seed`, `users` | Les adresses changent et se partagent |
| 2026-11-18 | Fenêtre du serveur (fonctionnalité `gui`, `--nogui`) : boîte de démarrage (monde existant ou nouveau, choix gardés dans `server-launch.toml`), carte, console, mesures, joueurs ; elle agit par des commandes de console ; sous Windows, relancée sans terminal | Lancer le serveur par son exécutable |

## Joueur et mouvement

| Date | Décision | Raison |
|---|---|---|
| 2026-11-18 | Le serveur fait autorité, avec prédiction et réconciliation côté client ; physique commune dans `nascio-physics` (dépend de `core` et `chunk`), pas fixe de 1/60 s, collision de boîte axe par axe, bits identiques client/serveur (empreinte de trajectoire testée) | Réconcilier seulement les vrais écarts |
| 2026-11-19 | Paramètres (`[player]`, envoyés dans `ServerHello`) : boîte 0,6 × 1,8 m, yeux 1,62 m ; marche 4,3, course 5,6, accroupi 1,3 m/s ; gravité 28 m/s², saut 1,25 m, chute ≤ 60 m/s ; marche de 0,5 m, jamais un bloc entier (saut automatique en option, décoché) ; nage 2 m/s | Saut de 0,6 s : chute nerveuse |
| 2026-11-18 | Touches : course = deux appuis sur Z à moins de 300 ms (arrêt au relâchement) ; accroupi = Maj ; champ de vision 42°, 60° en course et en vol | Décision de l'auteur |
| 2026-11-19 | Mode dieu réservé aux administrateurs (G, `ClientMsg::God`, confirmé par `ServerMsg::God`, vérifié par le serveur), éteint à chaque connexion ; blocs illimités ; vol seulement en mode dieu, par deux appuis sur Espace (sans effet en marche) ; vitesse fixe 20 m/s ; sans traverser les blocs ; se poser ou quitter le mode dieu rend la marche | Décision de l'auteur : l'inventaire ne se contourne pas |
| 2026-11-19 | Inventaire de marche tenu par le serveur : 10 slots de 32 blocs ; un bloc cassé va dans le premier slot de ce bloc non plein, sinon le premier vide, sinon il est perdu ; une pose vient du slot choisi (`SetBlock.slot`) ; envoyé entier (`ServerMsg::Inventory`), gardé par nom de bloc dans le compte au départ et toutes les `position_save_s` ; vide à la création | Décision de l'auteur |
| 2026-11-18 | Commandes numérotées en datagrammes (≤ 32 non acquittées répétées) ; `PlayerState` chaque tick en `f64` exacts ; crédit de 3 pas par tick (≤ 9, ≤ 4 par tick) ; famine = aucun datagramme pendant 250 ms → pas fabriqués sans touche ; commande sur cellule inconnue gardée en file | Rejeu identique ; pas d'accélération ni de triche par coupure |
| 2026-11-18 | Client : prédiction sur `Instant`, rejeu des commandes non acquittées à chaque état ; lissage τ = 100 ms, recalage au-delà de 2 m, à la mort, à la réapparition, au changement de mode ; après un acquittement au-delà de sa dernière commande, numérotation à `ack + 1 + ⌈RTT × 60⌉ + 3` ; états antérieurs à `Respawned` ignorés | Un seul recalage après un gel |
| 2026-11-18 | Éditions vérifiées par la simulation : portée 5 m des yeux après la commande `cmd` ; refus d'un bloc dans un joueur vivant ou d'un mort ; affichées seulement une fois renvoyées par le serveur | Exact quelle que soit la latence |
| 2026-11-18 | Mort : étouffement (enfermé par une édition, bloc solide au-dessus de la tête) et contact du magma ; `Died`, `Respawn`, `Respawned` ; réapparition au point du compte | Spec |
| 2026-11-18 | Sites d'apparition : 10 sites tirés de la graine au premier démarrage, gardés dans `spawn_sites.toml` (terre ferme sans eau, sous 1 000 m, hors bande polaire `s ≤ S_CLEAR`) ; point par compte à ≤ 300 m d'un site (`spawn_radius_m`) ; `spawn_xz` facultatif force un point commun | Monde inchangé ; tests et charges concentrés |
| 2026-11-18 | Connexion à la dernière position (sauvée au départ, à l'arrêt et toutes les 60 s) ; placement à la place libre la plus proche à ≤ 10 m (`free_place`, inconnu = pris) ; sans place : mort à la dernière position, nouveau point tiré autour du site (8 essais) au point du compte | Pas de joueur coincé ni de boucle de morts |

## Rendu et client

| Date | Décision | Raison |
|---|---|---|
| 2026-11-15 | Rendu relatif à une origine entière recentrée au-delà de 1 024 m, entités au `delta` torique | Coordonnées f32 petites, couture comprise |
| 2026-11-15 | Génération et maillage sur un pool rayon dédié (moitié des cœurs) | Pool de Bevy plafonné à 4 fils |
| 2026-11-15 | Horizon : 5 niveaux de 192×192 quads (16 à 256 m, ≈ 25 km), centre commun, trou du niveau 0 pour les chunks affichés, jupes hors de l'eau ; ignore les éditions des joueurs | Pas de recouvrement ni de vide ; tuiles statiques |
| 2026-11-15 | Courbure d'affichage `d²/(2·R_vis)` dans les shaders, `R_vis` = 200 km, fixe en jeu (plus de réglage « Courbure » ; `--curvature-radius` en développement, 0 = plat) ; normales plates, soleil local calculé par pixel (`sun.wesl`) | Impression de planète sans toucher la physique |
| 2026-11-16 | Ombres en cascades (3, 1,2 km) du soleil à la caméra ; `--no-shadows` | Le soleil local s'écarte de 4° au plus sur 1,2 km |
| 2026-11-16 | Données par image dans un `ShaderBuffer` partagé | Éviter de re-préparer tous les chunks |
| 2026-11-17 | Une seule fonction d'air (`atmosphere.wesl`) pour le dôme, la brume et le bord de l'horizon, avec le même tone mapping ; pas de brouillard de distance ; atmosphère selon l'altitude de la caméra (jusqu'à l'orbite) ; sol lointain texturé au-delà de l'horizon | Raccord ciel–relief |
| 2026-11-16 | Ciel : dôme, Soleil, Lune texturée (NASA CGI Moon Kit), étoiles BSC5 jusqu'à 6,5 (curseur −1 à 6,5, 5 par défaut) en points ronds, aurores en rideaux | Spec §2.5–2.8 |
| 2026-11-17 | Eau : opacité selon la profondeur (0,4 sur un bloc, ≈ 0,97 au large) | Rivières claires, mer opaque |
| 2026-11-18 | Neige saisonnière peinte sur les faces tournées vers le haut seulement ; limite des neiges par l'altitude fine | Pas de flancs enneigés confondus avec le ciel |
| 2026-11-19 | Distance de vue des chunks 1 024 m par défaut ; en réseau, ≤ `chunk_radius_m − 64`. Rayon serveur `chunk_radius_m` 1 088 m par défaut, un maximum : le client demande sa distance de vue + 64 m (`ClientMsg::ViewRadius`, protocole v14, 64 m au moins) et le serveur oublie les colonnes au-delà | Décision de l'auteur ; une courte distance de vue ne coûte rien au serveur |
| 2026-11-18 | Caméra aux yeux (`eye_height`) | Le serveur donne les pieds |
| 2026-11-16 | HUD : température, humidité relative, pression ; horloge solaire locale et de référence (λ = 0) | Demande de l'auteur |
| 2026-11-18 | Client : menu (Se connecter, Paramètres, Quitter), boîte de connexion (adresse, scan, nom, secret), Paramètres et menu Échap, panneau de débogage F3 ; réglages dans `client-settings.toml` à côté de l'exécutable (secret en clair) ; pas de carte ; `Bye` au départ | Un `.exe` autonome |
| 2026-11-19 | Thème des menus : fond blanc, titre « NASCIO » en Stentiga 80 px, boutons #FFCCA0 350 × 40 (survol #FFA353, appui #FF8519), un tous les 70 px, texte #323232 en Prototype ; mise en page pour 1 280 px mise à l'échelle (`viewer/src/theme.rs`) ; chat, HUD et panneaux en style egui | Identité visuelle de l'auteur |
| 2026-11-19 | Barre de 10 slots en bas de l'écran (icône = texture de face latérale, sinon couleur ; nombre en coin) ; molette = curseur ; nom du bloc 5 s au-dessus de la barre au changement de slot, aucun autre message ; croix au centre ; mode dieu : dirt, grass, regolith, gravel, spruce_log, spruce_leaves, snow, sand, sandstone, magma ; vol libre local (`--fly`) : G et Échap, portée 20 m, `flush` après chaque édition | Décision de l'auteur |
| 2026-11-19 | Feuillage ajouré : solide pour la physique, découpé par l'alpha de sa texture (seuil 0,5, matériau `AlphaMode::Mask`, ombres découpées aussi), faces intérieures entre feuilles dessinées, mips qui gardent la couverture ; seulement à moins de `[render] foliage_cutout_chunks` colonnes de la caméra (4 par défaut), opaque au-delà (colonnes remaillées au passage du rayon) | Décision de l'auteur ; les faces intérieures doublent les quads d'une forêt |
| 2026-11-19 | Textures : `bloc.png` (toutes faces), `_side`, `_top`, `_bottom`, `_end` (priorité `_top`/`_bottom`, `_end`, `_side`, base) ; dossier `textures/` à côté de l'exécutable (`--textures`, `none`) ; source `textures_temp/` hors dépôt, copiée par `scripts/sync-textures.ps1` ; teintes : herbe selon le climat, aiguilles vert fixe, eau ; le serveur et le protocole les ignorent | Essais visuels sans images dans le dépôt |
| 2026-11-19 | Champ de vision réglable dans les Paramètres (« Champ de vision », 40° à 100°, 60° par défaut, `fov_deg` de `client-settings.toml`) ; course et vol : 1,3 fois ce réglage (remplace 42° / 60°) | Demande de l'auteur |
