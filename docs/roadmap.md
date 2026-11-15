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

## M4 — Climat, saisons, biomes, ciel
- Climat (§4.3) avec stockage en harmoniques, hydrologie finale (§4.4), biomes (§4.5).
- Chunks générés jusqu'au Moho (pierre générique), puis magme du manteau ; la vraie colonne de roches vient en M5.
- Visualiseur : couches climatiques animées selon la date, superposition jour/nuit.
- Ciel 3D : course du soleil, crépuscule et couvert nuageux de la couture, anneau d'aurores, Lune et étoiles, neige saisonnière en surface.

**Validation** : des cartes de température et de pluie plausibles en janvier et en juillet, des déserts subtropicaux, des forêts équatoriales, une sécheresse visible à l'abri des montagnes, une bande polaire sombre sans saut du soleil.

## M5 — Géologie et sous-sol
- Colonne géologique (§4.6), Moho variable, température en profondeur, magme mortel, gisements par contexte.
- Visualiseur : coupe verticale (2D) et couche des « gisements ».

**Validation** : une coupe à travers une zone de subduction et à travers un orogène montre les bonnes roches, la bonne profondeur du Moho et les bons gisements.

## M6 — Finition du générateur de chunks
- Biomes et climat dans les chunks (sol, végétation provisoire, neige, glace), eau des rivières et des lacs.
- Préparation pour le serveur : implémentation du stockage des deltas (`generate ⊕ delta`), mesures de performance (chunks par seconde).

---
Ensuite viendra la **Phase « Serveur et réseau »**, qui fera l'objet d'une feuille de route séparée.
