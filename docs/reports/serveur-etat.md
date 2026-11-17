# État du travail — Phase « Serveur et réseau »

À relire après chaque compactage, avec `CLAUDE.md` et `docs/roadmap-serveur.md`.

**Consignes de la nuit (2026-11-18)** : étapes dans l'ordre (C0, S0, S1, S2…) ; décisions dans `decisions.md` (raison + alternative écartée) ; rapport court `docs/reports/serveur-<étape>.md` par étape ; un commit par étape, **pas de push** ; spec inchangée (texte proposé dans `docs/reports/spec-proposee.md`) ; ne pas décider ce qui touche au gameplay, aux invariants de `CLAUDE.md` ou à `generator_version` (le noter, continuer) ; tests verts avant l'étape suivante ; ce fichier à jour à chaque commit. Le matin : synthèse de tous les rapports.

## Étape en cours
S1 — durabilité des deltas (journal global).

## Fait
- Roadmap serveur écrite (`docs/roadmap-serveur.md`).
- C0 fait (commit « fix(voxel): durable delta writes ») : `write_durable`, journal créé atomiquement, `DeltaStore::sync`. Rapport `serveur-C0.md`.
- S0 fait : bin `netbench` (codecs, fsync), rapport `serveur-S0.md`, texte proposé `spec-proposee.md` (§5 journal global, §7 réseau).

## Prochaine action
S1 : implémenter dans `crates/voxel/src/delta.rs` le journal global décrit dans `docs/reports/spec-proposee.md` §5 (segments `deltas/wal/w.<lsn>.log`, LSN + CRC-32C, `flush` = un fsync, points de reprise, région format 2 avec LSN, migration des anciens `.log`), tests de crash, puis mesure du débit avec `netbench`.

## Décisions prises
- C0 : écriture durable (tmp, fsync, rename, fsync dossier Unix) ; pas de fsync dossier sous Windows.
- S0 : codec v2 = varint couches + zstd-3 par colonne ; crate zstd ; fsync groupé 50 ms ; budget 128 Ko/s par client.

## Pièges rencontrés
- PowerShell `Get-Content`/`Set-Content` cassent l'UTF-8 : écrire avec l'outil Write/Edit.
- Les chunks de `chunk_range` sont à 85 % uniformes : mesurer le non uniforme à part.
- Python via Bash : `open(p,'w',encoding='utf-8',newline='
')` pour garder LF.
