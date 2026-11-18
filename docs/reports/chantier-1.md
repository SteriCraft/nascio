# Chantier 1 — crate `nascio-chunk` (invariant 5)

But : que la compilation garantisse que le client ne peut pas générer de terrain. Le client dépend désormais d'une crate qui contient les types de chunk sans aucun code de génération. Il reçoit la table des blocs du serveur.

`generator_version` n'est pas touché : seules des définitions de types ont changé de crate. Les tests `golden` et `determinism` restent verts, et l'empreinte de génération ne lit pas la table des blocs.

## Fait
**1. La crate `crates/chunk` (`nascio-chunk`)**, commit `2d18026`. Elle dépend seulement de `nascio-core`, `serde`, `anyhow` et `toml`. Elle contient :
- `block` : `BlockDef`, `Matter`, `BlockRegistry` (`builtin`, `id`, `validate`, TOML), `BUILTIN`, `BlockId`, `AIR`, `srgb_to_linear`, et `Blocks`, la table lue par le maillage (matière, couleur linéaire, feuillage). Le feuillage est déduit du nom (`FOLIAGE` = `spruce_leaves`), comme avant ;
- `chunk` : `Chunk` (avec son codec v1 des fichiers de région), `PaddedChunk`, `N`, `NP`, `VOLUME` ;
- `tint` : `ChunkTint`. Son calcul (`Terrain::chunk_tint`) reste dans `voxel` ;
- `mesh` : le maillage glouton. Le maillage de référence, cellule par cellule, y est public (caché de la documentation), pour que `voxel` continue de comparer les deux sur du terrain généré.

Ce qui reste ailleurs :
- `ChunkDeltas` et `NoDeltas` restent dans `voxel` : c'est l'interface génération ⊕ delta.
- `BlockRegistry::with_catalog` devient `nascio_world::blocks::registry_with_catalog`, car elle lit le catalogue des gisements.
- Dans `voxel`, `Blocks` enveloppe la table avec les `ColumnIds` du générateur. Grâce à `Deref`, `blocks.ids.grass` et `mesh_chunk(&p, &blocks, …)` s'écrivent comme avant.

**2. Le protocole v5**, commit de cette sous-étape :
- `ServerHello.blocks` porte maintenant les définitions des blocs : id, nom, couleur, matière et densité. Pour les 55 blocs du monde de référence, cela fait environ 1 Ko, une fois par connexion.
- `nascio-client` construit et valide `Connection::blocks` à partir de cette table, et en tire l'id maximal accepté par le décodeur.
- `nascio-protocol` et `nascio-client` dépendent de `nascio-chunk` et plus de `nascio-voxel`. Le codec v2 et `Column` restent dans `protocol`.
- Visualiseur `--connect` : `remote.rs` n'importe plus que `nascio-chunk`, `nascio-client` et `nascio-protocol`, et il maille avec la table reçue. Il vérifie encore que cette table est celle du monde local, parce que l'horizon et les blocs du mode dieu viennent toujours du monde local, jusqu'au chantier 4.
- `swarm` et les tests `edit.rs` et `mesh.rs` lisent la table reçue. `mesh.rs` maille désormais le côté réseau avec la table du client : le maillage reste identique au maillage local.

**3. Garde-fou** : `crates/client/tests/no_generation.rs` lit `cargo tree -e normal` et vérifie deux choses :
- `nascio-client`, `nascio-protocol` et `nascio-chunk` ne dépendent ni de `nascio-world`, ni de `nascio-worldgen`, ni de `nascio-voxel` ;
- `nascio-chunk` ne dépend ni de `zstd` ni d'une crate de bruit.

## Vérifications
- `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings` et `cargo test --workspace` passent. `mesh.rs` (maillage réseau = maillage local) est vert, ainsi que `golden` et `determinism`.
- Serveur sur le monde de référence et visualiseur en `--connect --screenshot` (`out/chantier1-net.png`) : 613 colonnes prêtes, 0 manquante, 96 k quads. Le serveur a construit et envoyé 622 colonnes. C'est la même scène qu'en S4b : la nuit, en mer, au point d'apparition.
- Un client ou un serveur v4 est refusé à la poignée de main, avec un message qui indique la différence de version.

## Limite
Le visualiseur reste un outil de développement. Il dépend toujours de `voxel` et de `worldgen`, donc le compilateur ne surveille pas `remote.rs`. La garantie porte sur `nascio-client`, sur lequel le futur client autonome sera construit.

## Texte proposé pour `CLAUDE.md` (non modifié)
Dans « Architecture », ajouter :
```
  chunk/      nascio-chunk     types de chunk sans génération : table des blocs, chunks 32³,
                               teinte, maillage (ce que le client lit)
  protocol/   nascio-protocol  messages, codec de colonnes v2, trames, entités (sans réseau)
  server/     nascio-server    serveur QUIC : simulation 20 Hz, colonnes, éditions, comptes
  client/     nascio-client    connexion au serveur, miroir des colonnes, entités
```
Dans le même tableau, remplacer la ligne de `voxel` par :
```
  voxel/      nascio-voxel     génération de chunks (terrain, géologie, végétation), deltas
```
Puis supprimer la ligne « Plus tard : `server`, `protocol`, `client`, `web`. » ou la réduire à « Plus tard : `web`. ».

Remplacer le paragraphe « Sens des dépendances » par :
> Sens des dépendances : `core` ← `chunk` ← `world` ← `worldgen` et `voxel` ; `protocol` et `client` ne dépendent que de `core` et `chunk` ; `server` dépend de `voxel` et `protocol` ; `tools` et `viewer` dépendent de tout. `voxel` ne dépend pas de `worldgen` (sauf pour ses tests). **`core` ne dépend d'aucun moteur graphique.** **`client` ne dépend d'aucune crate de génération** (`world`, `worldgen`, `voxel`), ce que vérifie `crates/client/tests/no_generation.rs`.

Enfin, compléter l'invariant 5 par :
> Le client ne générera jamais de terrain : il dépend de `nascio-chunk` et jamais des crates de génération.
