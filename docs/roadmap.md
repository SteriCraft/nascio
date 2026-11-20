# Feuille de route — Phase « Monde »

Chaque jalon se termine par une revue avec l'auteur. On ne passe pas au jalon suivant sans validation.

## M0 — Fondations
- Workspace Cargo (`core`, `worldgen`, `voxel`, `viewer`), `.gitignore` (`target/`, `out/`), formatage et clippy.
- CI GitHub Actions : fmt, clippy et tests sous Windows et Linux.
- `nascio-core` :
  - `torus` : wrap, delta, distance, versions i32 et f64 ;
  - coordonnées de bloc et de chunk ;
  - temps du jeu ;
  - astronomie (φ, λ, δ, δ_eff, e, e_vis, azimut, Q).
- Tests :
  - propriétés sur la couture ;
  - continuité de `e` et de `e_vis` à travers la couture ;
  - garanties de la bande crépusculaire (§2.5).

**Validation** : `cargo test` passe au vert ; un petit CLI ou un test affiche les courbes de hauteur du soleil sur 24 h pour plusieurs valeurs de s.

## M1 — Tectonique et visualiseur 2D
- Tectonique (§4.1), points chauds compris, à pleine et à basse résolution.
- Export des couches en `.bin` et en PNG.
- Visualiseur, mode Carte : sélection de couche, répétition périodique, graticule, infobulle.
- Test de déterminisme (même graine, même hachage).

**Validation** : plusieurs graines donnent des continents variés, des chaînes de montagnes aux collisions, des dorsales, des fosses, des archipels de points chauds, et une couture invisible.

## M2 — Premier terrain 3D et horizon
- Registre de blocs minimal (`u16`, couleurs provisoires) et `generate_chunk` pure et déterministe. L'altitude vient de la bicubique périodique de la grille tectonique de 160 m (provisoire jusqu'à M3), plus un bruit de détail périodique selon le contexte. Colonne simple : herbe, terre, pierre, sable, eau jusqu'à y = 0. L'interface `generate ⊕ delta` est déclarée, sans implémentation.
- Visualiseur, mode 3D :
  - caméra libre (5 m/s à 2 km/s) et téléportation ;
  - génération asynchrone, maillage glouton, rendu relatif à la caméra, éclairage simple ;
  - affichage de la position, de φ, λ et s ;
  - distance de vue de 512 m par défaut.
- Horizon : niveaux de détail en maillage de hauteurs au-delà des chunks, sur plusieurs dizaines de km.
- Compteur des temps CPU et GPU par image, et mode benchmark scripté.

**Validation** :
- on traverse les coutures est-ouest et nord-sud sans artefact, ni dans les chunks ni dans l'horizon ;
- aucune couture visible entre les chunks et l'horizon ;
- 60 i/s en 1080p, réglages maximum, sur le portable de référence (benchmark).

**Mesure de performance (2026-11-15)** : critère atteint sur le portable de référence (RTX 3050 Mobile, Linux) avec `bench/default.toml`. Sur 32 545 images, la moyenne est de 3,84 ms (260 i/s), le p95 de 5,92 ms, le p99 de 7,16 ms et le 1 % low de 116 i/s. Seules 2 images dépassent 16,7 ms (max 25,9 ms). La traversée à 2 km/s est limitée par le CPU (chargement des chunks, GPU sous 1 ms). Le passage le long de la haute chaîne est la phase la plus lourde pour le GPU (3,7 ms en moyenne).

## M3 — Érosion et hydrographie
- Érosion de type stream power sur la grille de 32 m, priority-flood, lacs, première ébauche de rivières.

**Validation** : les vallées et les bassins versants sont crédibles, et les rivières atteignent la mer ou un lac.

**Notes issues de M1** :
- la ligne de lacs en pointillés de la graine 7 (chenal de rift dans le grand continent) : à transformer en vrais lacs ou en vallée avec l'hydrologie ;
- la tranche −200..0 m presque vide dans l'histogramme hypsométrique : le plateau, très étroit (1,2 km en 1/5), et le premier anneau du talus laissent un creux ; à reprendre avec l'érosion et la sédimentation côtière.

**Réalisé (2026-11-16, à valider en revue)** :
- grille fine de 32 m (3 125², ou 625² en 1/5) : suréchantillonnage bicubique, bruit de détail selon la rugosité, rifts en graben continu ;
- précipitations provisoires (zonales, orographiques le long des vents zonaux, bande polaire) ;
- 1 Ma d'érosion : stream power implicite, diffusion de Roering, cônes de sédiments côtiers ;
- lacs (dépressions d'au moins 2 m et 20 000 m²), directions D8, débits, bassins par exutoire, rivières (débit ≥ 0,1 m³/s, ordre de Strahler) ;
- chunks et horizon sur la grille érodée, avec l'eau des lacs ;
- graines 42, 7, 1234 et 2026 : génération complète en pleine résolution en 48 à 60 s (érosion 41 à 53 s), 70 à 115 lacs par monde, terre abaissée de 37 à 68 m en moyenne ;
- ligne de lacs de la graine 7 : devenue une vallée de graben avec des lacs ; creux −200..0 m : comblé par les cônes de dépôt (voir `hypsometry.png`).
- benchmark `bench/default.toml` sur la graine 42 érodée (RTX 5060, Windows) : p95 de 4,29 ms, moyenne de 2,96 ms (338 i/s), 1 % low à 144 i/s ; aucune régression par rapport à M2. À refaire sur le portable de référence.

## M4 — Climat, saisons, biomes, ciel
- Climat (§4.3) avec stockage en harmoniques, hydrologie finale (§4.4), biomes (§4.5).
- Chunks générés jusqu'au Moho (pierre générique), puis magme du manteau ; la vraie colonne de roches vient en M5.
- Visualiseur : couches climatiques animées selon la date, superposition jour/nuit.
- Ciel 3D : course du soleil, crépuscule et couvert nuageux de la couture, anneau d'aurores, Lune et étoiles, neige saisonnière en surface.

**Validation** : des cartes de température et de pluie plausibles en janvier et en juillet, des déserts subtropicaux, des forêts équatoriales, une sécheresse visible à l'abri des montagnes, une bande polaire sombre sans saut du soleil.

**Réalisé (2026-11-16, à valider en revue)** :
- revue de M3 : soulèvement tectonique pendant l'érosion, tiré des vitesses de convergence (sutures soudées comprises) ;
- climat sur la grille de 160 m (24 pas par an, harmoniques) : température (profil calé, inertie terre/mer, continentalité, gradient vertical, courants), vents à trois cellules suivant `δ_eff`, humidité advectée et pluies orographiques, ZCIT et ceintures sèches, nuages, neige au sol, bande polaire climatique (pluies fortes, neige permanente) ; amplitude diurne ;
- hydrologie finale avec les pluies du climat (largeur et profondeur des rivières) ; biomes Köppen sur la grille de 32 m (14 codes) ;
- chunks jusqu'au Moho (pierre générique), magma dessous ;
- côtes en escalier de 160 m corrigées (distorsion de domaine) ;
- visualiseur : cartes climatiques animées selon la date, jour/nuit par pixel ; en 3D, soleil calculé par pixel, ombres dans le shader courbé, dôme du ciel, Lune et ses phases, étoiles réelles, aurores, couvert polaire, neige saisonnière ; `R_vis` = 200 km ;
- graines 42, 7, 1234 et 2026 : génération complète en 61 à 73 s (climat 7,5 s) ; benchmark : p95 6,22 ms avec les ombres, 4,43 ms sans (M3 : 4,29 ms).

**Retouches après la revue de M4 (2026-11-16)** :
- relief marin : abysse à −1 800 m, fosses à −2 500 m, talus de 5 km sur toutes les marges, pente du fond bornée à 0,35 et pente côtière à terre bornée à 0,8 (plus de parois ni de murs de cônes) ; écart à l'hypsométrie terrestre consigné au §3 ;
- continentalité sur 20 km avec une saison amortie en mer ; soulèvement maximal de 0,5 mm/an ;
- rendu : neige sur les faces verticales et limite des neiges par l'altitude fine, étoiles du BSC5 jusqu'à la magnitude 6,5 (curseur, 5 par défaut), Lune texturée (NASA SVS), FOV de 42°, minimap, point d'apparition choisi sur la carte, vsync, horloge locale et d'origine, distance des chunks de 1 024 m par défaut ;
- graines 42, 7 et 1234 : génération en 80 à 93 s ; benchmark sur la graine 42 : moyenne 3,52 ms, p95 4,78 ms (M4 : 3,72 et 6,22 ms).

**Atmosphère et pluies par saturation (2026-11-16)** :
- pression atmosphérique selon le profil terrestre comprimé ×2, nulle à 50 km (472 hPa à 3 km) ; gradient thermique conservé à −6,5 °C/km ;
- humidité de l'air (rapport de mélange) advectée et exportée ; pluies par condensation au-delà de la saturation `w_s(T, P)` ;
- profil thermique réchauffé vers 45–60° : juillet de 14 à 17 °C sur les côtes, mer libre jusqu'à 55° ;
- plateformes de dépôt en pente depuis la côte : de −1 m au rivage jusqu'à −20 m environ à 1 km ;
- HUD 3D : température, humidité relative et pression à la caméra, sous l'horloge ;
- graines 42, 7, 1234 et 2026 : génération en 87 à 136 s (`generator_version` 8) ; benchmark sur la graine 42 : moyenne 3,49 ms, p95 4,77 ms (sans changement).

## M5 — Géologie et sous-sol
- Colonne géologique (§4.6), Moho variable, température en profondeur, magme mortel, gisements par contexte.
- Visualiseur : coupe verticale (2D) et couche des « gisements ».

**Validation** : une coupe à travers une zone de subduction et à travers un orogène montre les bonnes roches, la bonne profondeur du Moho et les bons gisements.

## M6 — Finition du générateur de chunks
- Biomes et climat dans les chunks (sol, végétation provisoire, neige, glace), eau des rivières et des lacs.
- Préparation pour le serveur : implémentation du stockage des deltas (`generate ⊕ delta`), mesures de performance (chunks par seconde).

**Plan validé (2026-11-17)**. Une étape à la fois ; à la fin de chaque étape, un court rapport, puis l'auteur vérifie dans le visualiseur. Les vérifications approfondies se font à la fin du jalon et à l'audit.
1. **Sol selon le biome** — fait. L'herbe est un seul bloc partout : sa couleur varie selon le climat, avec des transitions en dégradé (même une fois texturée).
2. **Neige et glace permanentes** — fait : bloc `ice` (teinté de bleu) pour les glaciers et la banquise (mer et lacs) ; bloc `snow` (blanc) pour la neige permanente ; une **surcouche de neige** fine qui peut s'accumuler (à la manière de Minecraft). La neige saisonnière reste un effet de rendu jusqu'à la météo.
3. **Eau des rivières** — fait : tracé lissé le long de `flow_direction` (distances par `torus::delta`), chenal creusé (largeur et profondeur des couches), surface de l'eau qui ne remonte jamais vers l'aval, paliers sur les fortes pentes, raccord avec les lacs et la mer. De l'eau à partir d'environ 1,5 m de large ; vers l'aval, un ruisseau garde au moins la plus petite largeur atteinte jusqu'à son lac ou à la mer. Mêmes blocs d'eau que la mer, transparents et teintés de bleu, plus sombres et plus opaques avec la profondeur. Pas d'animation ni d'écoulement.
4. **Végétation provisoire** — fait : un seul modèle d'arbre fixe, de type « sapin », quel que soit le biome ; des cactus de 3 blocs dans le désert chaud. Points de départ des troncs procéduraux et déterministes (hachage des coordonnées du monde), densité selon le biome ; un arbre peut déborder sur le chunk voisin. Une liste d'arbres par biome est prévue, à remplir plus tard (étape « Végétation » dédiée).
5. **Stockage des deltas** — fait : `DeltaStore` (blocs modifiés en valeur absolue, structure triée, palette au-delà d'un seuil), fichiers de région dans `world-<seed>/deltas/` avec la version du générateur et l'empreinte du monde ; outil de pose et de destruction dans le visualiseur (futur « god mode »).
6. **Performance** — fait : binaire `chunkbench` (`cargo run -p nascio-tools --release --bin chunkbench -- out/m6/world-42`), génération et maillage sur 1 et N cœurs. Pas de cible fixée. Référence du 2026-11-17 (monde 42 d'échelle 5, grille fine de 160 m ; 61 colonnes/s sur 1 cœur à l'échelle 1 selon l'audit ; 8 sites de 4×4 colonnes, 12 cœurs) : 65 colonnes/s sur 1 cœur, 524 colonnes/s sur 12 (×8) ; par chunk, 4,3 ms de génération et 2,7 ms de maillage, plus 3,6 ms par colonne.
7. **Vérification et clôture** — fait (2026-11-17, à valider en revue) : binaire `topview` (vues de dessus en PNG dans `out/m6/topview/`), captures 3D dans `out/m6/captures/` (rivière, montagne, plaine près du grand lac), `generator_version` inchangée (14), spec (§5, outils de mesure) et décisions à jour. Quatre anomalies relevées, non corrigées (voir `decisions.md`) : limite droite de la banquise, chenal gelé sur la banquise, limite de roche alignée sur un bord de chunk, rivières à un seul bloc d'eau.

## Audit — Revue complète avant la phase « Serveur et réseau »
Audit complet du projet par Fable, à partir des derniers benchmarks du PC Windows (RTX 5060) et du portable Linux de référence (RTX 3050 Mobile). Il couvre :
- le déterminisme et le respect des invariants de `CLAUDE.md` (tore, coordonnées entières, RNG, bruit périodique) ;
- la performance de la génération et du maillage des chunks ;
- l'architecture des crates et le sens des dépendances ;
- la dette des solutions provisoires consignées dans `decisions.md` ;
- le rendu (shaders, éclairage, ciel, horizon).

**Validation** : un rapport priorisé, trié en revue avec l'auteur avant d'ouvrir la phase « Serveur et réseau ».

**Fait (2026-11-17)** : rapport `rapport-audit-M7.md` (0 critique, 6 élevés, 11 moyens, 9 faibles), validé par l'auteur. Tous les constats sont appliqués avant la phase Serveur, en cinq lots, un commit par lot (lot 5 : un commit par sous-lot) :
1. **Déterminisme** — fait et validé (2026-11-17) : empreinte de génération (monde + `[voxel]`), `--dev-override` marqué, `libm` pour le code de génération, hachages de référence (monde au 1/25, 24 chunks), CI Linux et Windows ; `generator_version` 15. Monde de référence d'échelle 1 : `out/m7/world-42` (101 s).
2. **Rivières et anomalies** — fait et validé (2026-11-17) : gabarit de jeu par ordre de Strahler, méandres, vallées sans gorges, embouchures, pas d'eau sous la glace ; banquise tramée ; altération amincie et tramée (cause de l'anomalie c prouvée) ; `generator_version` 16. Vues avant/après dans `out/m7/lot2/`.
3. **Performances** — fait et validé (2026-11-17) : données de roche et bruits par colonne, gauchissement et biome dédoublonnés (sortie identique), pools mesurés et écartés ; table de gradients de Perlin, `generator_version` 17, qui rattrape le coût de libm. Monde d'échelle 1 : 55 → 143 colonnes/s sur 1 cœur, 410 → 1 214 sur 12. Le monde de référence `out/m7/world-42` est régénéré en version 17 (121 s).
4. **Stockage des deltas** — fait et validé (2026-11-17) : régions verrouillées séparément et chargées à la demande, journal en ajout seul compacté dans le fichier de région (format conservé), instantanés par chunk, régions en mémoire bornées, `ColumnCache` borné, tests `proptest` à travers la couture. Une édition coûte 0,007 ms au lieu de 4,6 ms (1 million de blocs modifiés, un lecteur actif).
5. **Constats restants** : 5a crates — fait (2026-11-17, à valider) : crate de lecture `nascio-world` (dont `voxel` dépend seule), outils dans `nascio-tools`, mondes antérieurs à M5 refusés et chemins `Option` retirés du terrain, `torus::delta_chunks`, diviseurs mis en cache ; sortie identique ; 5b maillage — fait (2026-11-17, à valider) : faces trouvées par masques de bits (0,27 ms par chunk au lieu de 2,63, même maillage), chunks uniformes sans tableau, codec palette + plages ; 5c shaders — fait (2026-11-17, à valider ; bancs du portable Linux à refaire) : soleil local sous forme vectorielle (plus d'`atan2` ni de second `asin` par pixel hors de la bande de la couture), atmosphère et soleil en modules WESL partagés (`atmosphere.wesl`, `sun.wesl`) avec un test contre `core::astro`, canal de matériau en UV0 au lieu des étiquettes dans l'alpha ; rendu identique au pixel près hors de la ligne d'eau lointaine, 5d documentation — fait (2026-11-17) : décisions closes ou remplacées signalées (normales de la courbure, relaxation de talus, anomalies de M6), tests du climat, des biomes et des glaciers à l'échelle 1/5 (M11-1), ensemble ordonné pour le trou de l'horizon (F9). Références de `chunkbench` à l'échelle 1 (`out/m7/world-42`, version 17) : 143 colonnes/s sur 1 cœur, 1 214 sur 12 ; maillage 0,27 ms par chunk ; ligne verticale du site polaire (86 592, 0) — corrigée (2026-11-17) : gradin dû au bornage de la bicubique par cellule, remplacé par une bicubique monotone (`generator_version` 18) ; climat de la couture — recalé (2026-11-17) : la banquise pérenne suit l'air, moyenne zonale −18,8 °C pour une cible ≈ −20 °C, test actif (`generator_version` 19).

**Audit clos (2026-11-18)** : tous les constats sont appliqués ou classés (statut et commit de chacun dans `rapport-audit-M7.md`), CI verte sous Linux et Windows (`c960dad`). Références de performance en `generator_version` 19, monde d'échelle 1 `out/m7/world-42` (graine 42), PC Windows (12 cœurs, RTX 5060) :
- `chunkbench` (8 sites de 4×4 colonnes, médiane de trois passes) : 374 colonnes/s sur 1 cœur (1,42 ms par colonne, génération 0,37 ms et maillage 0,28 ms par chunk) ; 2 505 colonnes/s sur 12 cœurs (×6,7). Avant le lot 3 (même monde, version 16) : 55 colonnes/s sur 1 cœur et 410 sur 12 ;
- banc du visualiseur `bench/default.toml` (1920×1080, 25 700 images) : moyenne 3,84 ms (260 i/s), p95 6,07 ms, p99 8,00 ms, 1 % low 97,6 i/s, 4 images au-delà de 16,7 ms ; CPU 2,46 ms, GPU 1,17 ms en moyenne. Une première passe, perturbée par des blocages périodiques extérieurs au banc, donnait la même charge CPU et GPU (2,52 et 1,20 ms) mais un p95 de 16,18 ms : elle est écartée ;
- le portable Linux (RTX 3050 Mobile) sera re-mesuré plus tard, à la demande de l'auteur.

## Végétation
Remplace la végétation provisoire de M6 (un sapin, un cactus). Arbres à **taille réelle** ; deux essences au moins par biome qui porte des arbres. Une étape à la fois ; à la fin de chaque étape, un court rapport, puis l'auteur vérifie. Chaque étape qui change les chunks incrémente `generator_version` et met à jour la spec (§5, végétation) et `decisions.md`.

1. **Espacement des troncs** — fait (2026-11-20, à valider ; `generator_version` 21) : au moins un bloc entre deux troncs, diagonale comprise. Dans une cellule de côté `c`, un tronc de largeur `w` se place dans `[0, c − 1 − w]` : l'écart avec toute cellule voisine est alors d'au moins un bloc, sans comparer les cellules entre elles (couture comprise). Largeur de tronc de 2 au plus avec `c = 5`. **Validation** : test de propriétés (`proptest`) sur l'écart, couture comprise ; promenade de l'auteur.
2. **Couleur et relief du lointain** — fait (2026-11-20, à valider ; `generator_version` 22 ; outil `cargo run -p nascio-tools --release --example far_ground -- <monde> <dossier>`) : les tuiles d'horizon et le sol lointain (`ground_colors`) tiennent compte des arbres par une **couverture attendue**, calculée sans tirer les arbres : densité du biome × aire moyenne des couronnes de ses essences ÷ aire d'une cellule, nulle là où les arbres ne poussent pas (limite des arbres, pente, sable, gravier, rivières, eau), bornée à 1. La couleur mêle celle du sol et celle des feuillages selon la couverture ; la hauteur des tuiles peut être relevée d'une fraction de la hauteur des arbres × couverture (relief de canopée), à garder ou non après essai. **Validation** : passage des chunks aux tuiles sans saut de couleur visible, en forêt dense et en lisière.
3. **Modèles paramétriques et taille variable** : un modèle par forme (conique, ovale, ronde, parasol, plate, palmier, retombante…) et paramètres en TOML (hauteur visée, part du tronc, rayon de couronne, largeur du tronc, blocs) ; modèles construits une fois par essence et par hauteur. Facteur de taille tiré du hachage de la cellule dans [0,7 ; 1,3] (réglable par essence), qui s'applique à la hauteur et à la couronne ; orientation parmi quatre. Portée horizontale (`MAX_RADIUS`) déduite de la table. L'épicéa passe au nouveau format, à taille réelle. Densités par biome recalées sur les couronnes réelles. **Validation** : hauteurs dans ±30 % (test), `chunkbench` et octets par colonne mesurés (la portée des couronnes augmente le nombre de cellules lues par chunk).
4. **Zone d'essai** : option du serveur qui remplace la végétation dans un rayon d'environ 100 m autour de la position enregistrée d'un compte par une seule essence, sur une copie du monde dans `out/` ; paramètres de l'essence relus à chaque lancement. Le client s'y connecte et l'auteur s'y promène.
5. **Essences**, une par une, chacune essayée dans la zone d'essai puis validée ou retouchée par l'auteur ; deux blocs par essence (tronc, feuilles) ajoutés à `FOLIAGE` et à `docs/textures.md`.

   | Biome | Essences |
   |---|---|
   | Forêt boréale | épicéa, pin sylvestre |
   | Toundra (sous la limite des arbres) | épicéa, bouleau |
   | Forêt tempérée | chêne, bouleau |
   | Méditerranéen | pin parasol, olivier |
   | Steppe | chêne, pin sylvestre |
   | Savane | acacia, baobab (tronc 2×2) |
   | Forêt tropicale | fromager (tronc 2×2), palmier |
   | Mangrove | palétuvier, palmier |
   | Marais, lac | saule, bouleau |
   | Désert chaud | cactus (inchangé) |
6. **Arbres dans l'eau** : le palétuvier pousse dans une eau de 1 m de profondeur au plus.
7. **Dégradé entre biomes** : la fréquence d'une essence croît progressivement en entrant dans son biome, pour passer d'un type de forêt à l'autre sans limite nette.

**Plus tard** : saisons du feuillage (couleurs d'automne, arbres nus en hiver), avec leur effet sur la couleur du lointain.

## Reste à faire (phase « Monde »)
Les limites détaillées sont dans `docs/limites.md` (numéros L…). Cocher ou retirer une ligne quand elle est faite.
- [ ] Banc complet sur le portable Linux (RTX 3050 Mobile), aurores comprises (L8, `rapport-audit-M7.md` §9).
- [ ] Au prochain changement de format du monde : le feuillage devient une propriété de `blocks.toml` (L3).
- [ ] Spec, mode 3D : décrire les textures des blocs (la spec dit encore « les textures viendront plus tard » ; règle de nommage faite dans `docs/textures.md`) ; règles des faces et teintes dans un fichier TOML (L7).
- [ ] **Textures** — état : branche fusionnée dans `main` (602a932, 2026-11-18), usage de **test visuel** seulement ; 20 blocs sur 25 environ sont texturés par les images de l'auteur (`textures_temp/blocks`, hors dépôt, option `--textures`). Reste :
  - [ ] textures manquantes : `ice`, `snow_layer` (prend `snow` pour l'instant), les huit roches magmatiques et métamorphiques (`granite`, `gneiss`, `schist`, `basalt`, `gabbro`, `andesite`, `granulite`, `peridotite`), les minerais de `config/deposits.toml` (liste dans `docs/textures.md`) ;
  - [x] textures : pack 32×32 libre de droits de l'auteur (confirmé le 2026-11-19), pas de réécriture de l'historique ; `ASSETS.md` à compléter avec la source et la licence du pack.
  - [x] `docs/textures.md` coché (2026-11-19).
  - [x] point d'arrivée sans sol : disparu (confirmé par l'auteur le 2026-11-19).
- [ ] Tuiles d'horizon invalidées par région éditée, si de grands chantiers des joueurs doivent se voir de loin (L1).

---
Ensuite vient la **Phase « Serveur et réseau »**, suivie dans sa propre feuille de route, `roadmap-serveur.md` (jalons C0 à S7, puis les chantiers ; chantier 5, mouvement autoritaire, clos le 2026-11-19).
