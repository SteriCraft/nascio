# Rapport C0 — Correction de la compaction (2026-11-18)

## Fait
- `write_durable` (`crates/voxel/src/delta.rs`) : fichier temporaire `<nom>.tmp`, `fsync`, renommage, `fsync` du dossier sous Unix.
- La compaction l'utilise : le fichier de région est sur le disque (données puis nom) **avant** la suppression du journal.
- Un journal est désormais créé avec son en-tête en une seule écriture durable : plus d'en-tête déchiré, qui faisait échouer l'ouverture de tout le magasin.
- `DeltaStore::sync()` : `fsync` des journaux ouverts (pour une fermeture propre ; la validation groupée viendra à S1).
- Test `leftovers_of_an_interrupted_write_are_ignored` : des `.tmp` à moitié écrits n'empêchent ni l'ouverture ni la lecture.

## Mesures
Aucune (correction). Tests de l'espace de travail verts, clippy propre.

## Décisions
- Pas de `fsync` du dossier sous Windows (impossible avec `std` ; NTFS journalise le renommage).
- Pas de `fsync` par édition : la validation groupée est l'objet de S1.

## Questions pour l'auteur
- Aucune. Format des fichiers inchangé, `generator_version` inchangée.
