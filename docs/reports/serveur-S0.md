# Rapport S0 — Mesures (2026-11-18)

Outil : `cargo run -p nascio-tools --release --bin netbench -- codecs out/m7/world-42` (et `-- fsync out/netbench`). Monde de référence d'échelle 1, graine 42, `generator_version` 19 ; PC Windows (NVMe, 12 cœurs).

## Codecs de chunks
Chunks mesurés : toutes les couches de `ColumnSet::chunk_range` (ce qu'un serveur enverrait), 24 sites de 4 × 4 colonnes (384 colonnes, 112 sur terre ; 7 738 chunks dont **85 % uniformes**, surtout de l'eau et de la roche). Octets par chunk non uniforme, moyenne / médiane / p95 :

| Codec | Terre | Mer |
|---|---|---|
| v1 actuel (plages `u16`, ordre des couches) | 2 507 / 1 335 / 8 411 | 792 / 367 / 2 843 |
| plages varint, ordre des couches | 1 265 / 674 / 4 229 | 404 / 194 / 1 437 |
| plages varint, ordre des colonnes (y d'abord) | 6 502 / 6 317 / 13 616 | 6 219 / 6 149 / 10 267 |
| palette compactée par bits | 9 658 / 8 197 / 16 394 | 6 910 / 8 196 / 8 197 |
| v1 + zstd-3 | 607 / 392 / 1 882 | 198 / 116 / 679 |
| varint couches + lz4 | 783 / 485 / 2 607 | 244 / 115 / 905 |
| **varint couches + zstd-3** | **483 / 319 / 1 514** | **164 / 103 / 545** |
| palette par bits + zstd-3 | 477 / 323 / 1 497 | 147 / 106 / 435 |

Par colonne entière (tous les chunks de la plage, uniformes compris) : v1 3 866 o ; varint 1 986 o ; varint + zstd-3 par chunk 997 o ; **colonne compressée d'un bloc (varint + zstd-3) : 1 277 o sur terre, 468 o en mer** (48 sites, 768 colonnes).

Vitesses sur un fil, par chunk non uniforme : décodage varint 4,4 µs, décompression zstd 4,6 µs ; encodage v1 20 µs, encodage varint de l'outil 75 µs (prototype non optimisé : une recherche linéaire dans la palette par bloc ; le codec de S2 visera le coût de v1). L'encodage n'a lieu qu'une fois par chunk grâce au cache du serveur.

Constats :
- L'ordre « colonnes » est bien pire : sous la surface, les strates et les bruits de la géologie varient d'un bloc à l'autre le long de y bien plus qu'à l'horizontale.
- Les plages varint divisent v1 par deux ; zstd divise encore par 2,5 ; compresser une colonne entière d'un bloc gagne encore 30 %.
- LZ4 est moins bon que zstd-3 (environ 1,6 fois plus gros) pour un gain de vitesse inutile ici (décompression déjà à 5 µs).

## `fsync`
Fichier en ajout seul, enregistrements de 24 octets, 200 tours (NVMe, Windows) :

| Groupe | `fsync` médian | p99 | Éditions/s |
|---|---|---|---|
| 1 | 0,42 ms | 0,87 ms | 2 110 |
| 10 | 0,42 ms | 0,60 ms | 23 536 |
| 100 | 0,63 ms | 0,94 ms | 156 586 |
| 1 000 | 2,82 ms | 4,17 ms | 332 853 |

Sans `fsync` : 405 905 éditions/s. Un `fsync` par édition plafonne à environ 2 000 éditions/s et bloquerait le tick ; un `fsync` par tick coûte moins de 1 ms jusqu'à quelques centaines d'éditions. Sur un SSD de serveur Linux (ext4), l'ordre de grandeur est le même ou meilleur ; sur un disque réseau de VPS, il peut atteindre 5 à 10 ms : à re-mesurer sur l'hébergement réel avec la même commande.

## Modèle de débit d'une bataille (500 joueurs dans 200 m)
- **Entités** : environ 10 octets par mise à jour quantifiée ; les 64 plus proches à 20 Hz, les autres à 5 Hz : 64·10·20 + 435·10·5 ≈ 34 Ko/s par client.
- **Éditions** : 500 joueurs × 2 éditions/s = 1 000 éditions/s, environ 6 octets chacune, diffusées à tous : 6 Ko/s par client ; 50 enregistrements par tick dans le journal (un `fsync` < 1 ms par tick).
- **Surcoût UDP/QUIC** : environ 50 octets par paquet, 20 paquets/s : 1 Ko/s.
- **Total en régime** : environ 41 Ko/s par client (330 kbit/s) ; serveur : 500 × 41 ≈ 20 Mo/s ≈ **165 Mbit/s**.
- **Arrivée sur le site** : rayon de chunks de 512 m ≈ 800 colonnes ≈ 1 Mo sur terre (4 s à 256 Ko/s). Pour 500 joueurs, 500 Mo au total, absorbés en 4 s par un lien de 1 Gbit/s s'ils arrivent ensemble ; la génération des colonnes uniques du site (≈ 1 500) prend environ 0,6 s sur 12 cœurs (2 505 colonnes/s), une fois grâce au cache partagé. À 1 024 m de rayon (valeur du visualiseur), il faudrait 4 fois plus : 4 Mo par joueur.
- **Processeur** : 500² = 250 000 paires par tick évitées par une grille spatiale (cellules de 32 m) ; le coût principal reste la sérialisation, environ 500 × 500 états par seconde pondérés par la priorité.

## Décisions (détail dans `decisions.md`)
1. Codec de chunks v2 : plages varint dans l'ordre des indices (couches), uniforme en 2 à 3 octets ; compression zstd niveau 3 par lot (une colonne, ou ce qu'un message regroupe), faite une fois dans le cache du serveur.
2. Crate `zstd` (bibliothèque C de référence) plutôt qu'un zstd en Rust pur ou LZ4.
3. Validation groupée du journal : un `fsync` par intervalle (`fsync_interval_ms`, 50 ms par défaut, soit un tick).
4. Budget par client de 128 Ko/s par défaut (réglable) : les entités d'abord, les éditions ensuite, les chunks remplissent le reste.

## Questions pour l'auteur
- **Rayon de chunks servi** : je propose 512 m par défaut (réglable), avec l'horizon en tuiles au-delà. Le visualiseur montre 1 024 m ; le choix touche à l'expérience de jeu, je ne le fige pas.
- **Hébergement** : à confirmer (re-mesurer `netbench fsync` dessus ; le modèle suppose 1 Gbit/s).
- **Fréquence des entités lointaines** (5 Hz au-delà des 64 plus proches) : choix technique par défaut, mais il change la lisibilité d'une bataille ; à revoir en S5/S6 avec des bots.
