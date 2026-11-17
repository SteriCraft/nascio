# État du travail — Phase « Serveur et réseau »

À relire après chaque compactage, avec `CLAUDE.md` et `docs/roadmap-serveur.md`.

**Consignes de la nuit (2026-11-18)** : étapes dans l'ordre (C0, S0, S1, S2…) ; décisions dans `decisions.md` (raison + alternative écartée) ; rapport court `docs/reports/serveur-<étape>.md` par étape ; un commit par étape, **pas de push** ; spec inchangée (texte proposé dans `docs/reports/spec-proposee.md`) ; ne pas décider ce qui touche au gameplay, aux invariants de `CLAUDE.md` ou à `generator_version` (le noter, continuer) ; tests verts avant l'étape suivante ; ce fichier à jour à chaque commit. Le matin : synthèse de tous les rapports.

## Étape en cours
S0 — mesures.

## Fait
- Roadmap serveur écrite (`docs/roadmap-serveur.md`).
- C0 fait (commit « fix(voxel): durable delta writes ») : `write_durable`, journal créé atomiquement, `DeltaStore::sync`. Rapport `serveur-C0.md`.

## Prochaine action
S0 : bin `netbench` dans `nascio-tools` — tailles et vitesses des codecs de chunks sur `out/m7/world-42`, coût du `fsync` (seul / groupé), modèle de débit de bataille ; texte proposé §7 dans `docs/reports/spec-proposee.md`.

## Décisions prises
- C0 : écriture durable (tmp, fsync, rename, fsync dossier Unix) ; pas de fsync dossier sous Windows.

## Pièges rencontrés
- PowerShell `Get-Content`/`Set-Content` cassent l'UTF-8 : écrire avec l'outil Write/Edit.
