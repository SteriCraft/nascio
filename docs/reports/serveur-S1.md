# Rapport S1 — Durabilité des deltas (2026-11-18)

## Fait
- **Journal global** dans `crates/voxel/src/delta.rs` : segments `deltas/wal/w.<lsn>.log` (en-tête `NSCWAL`), enregistrements `longueur, LSN, type, contenu, CRC-32C`. Type 1 : édition de bloc (`x, y, z`, nom du bloc). Les autres types sont réservés aux futurs états de jeu.
- **Écriture** : `set` écrit l'enregistrement aussitôt dans le segment courant (il survit à l'arrêt brutal du programme), l'applique, et renvoie son LSN. `flush()` fait un seul `fsync` pour tout ce qui précède (validation groupée) et renvoie le dernier LSN durable. Le serveur l'appellera toutes les 50 ms ; le visualiseur après chaque édition ; `Drop` aussi.
- **Points de reprise** : au-delà de `checkpoint_bytes` (16 Mio), un nouveau segment est ouvert, les régions modifiées sont réécrites durablement avec leur LSN (fichier de région **format 2**), puis les anciens segments sont supprimés. Une région modifiée est réécrite avant d'être déchargée. Une région illisible garde les segments qui la concernent.
- **Chargement** : l'ouverture lit les en-têtes et parcourt le journal une fois (enregistrements indexés par région) ; une région se charge avec son fichier puis ses enregistrements de LSN supérieur au sien. Une fin de dernier segment invalide est retirée (problème signalé) ; une erreur ailleurs fait échouer l'ouverture.
- **Migration** : les fichiers de région de format 1 se lisent (LSN 0) ; les anciens `r.<rx>.<rz>.log` sont rejoués au premier chargement de leur région, puis la région est réécrite et le journal supprimé. Les deltas existants de `out/m7/world-42` seront migrés à la prochaine ouverture du visualiseur.
- `compact_after` est remplacé par `checkpoint_bytes` dans `DeltaConfig` ; `compact_all` devient un alias de `checkpoint`.
- **Tests** (13 → 18 dans `delta`) : fin de segment coupée, enregistrement corrompu (dernier segment : retiré ; avant : refus), migration du format 1, enregistrements déjà dans un fichier de région non rejoués (crash entre l'écriture de la région et la suppression des segments), et un **processus enfant tué** en pleine écriture : toutes les éditions annoncées comme durables sont retrouvées, la numérotation continue. Le `proptest` de la couture passe avec des points de reprise fréquents.

## Mesures (`netbench wal`, un fil d'édition, `flush` toutes les 50 ms sur un autre fil, NVMe Windows)
| Charge | Éditions/s | `set` médian / p99 / max | `flush` médian / p99 / max |
|---|---|---|---|
| Au plus vite (1,6 M éditions, plusieurs points de reprise) | 320 706 | 2,7 µs / 7,3 µs / 3,5 ms | 0,97 / 2,51 / 2,62 ms |
| 2 000 éditions/s (bataille de 500 joueurs × 4) | 2 000 | 5,8 µs / 42 µs / 1,9 ms | 0,52 / 0,95 / 0,97 ms |

Le maximum de `set` correspond à un point de reprise fait pendant l'appel. Réouverture après un point de reprise : 5,6 ms.

## Décisions (détail dans `decisions.md`)
- Journal global avec validation groupée plutôt que des journaux par région.
- Écriture immédiate de chaque enregistrement, plutôt qu'un tampon vidé au `flush` : la tenue à l'arrêt brutal du programme est conservée.
- Une fin de dernier segment invalide est retirée ; une erreur ailleurs fait échouer l'ouverture.

## Limites et suites
- Le point de reprise se fait dans l'appel de `set` qui dépasse le seuil (3,5 ms au pire ici, davantage avec beaucoup de régions modifiées). Au serveur (S3), il passera sur le fil d'écriture, à côté du `flush`.
- Le pire temps de démarrage (rejouer 16 Mio de journal) n'est pas mesuré ; estimation : quelques centaines de millisecondes.
- Pas de `fsync` du dossier sous Windows (voir C0).

## Questions pour l'auteur
- Valider le texte proposé du §5 (`docs/reports/spec-proposee.md`) avant de le reporter dans la spec.
