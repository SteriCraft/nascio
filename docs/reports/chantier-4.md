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
