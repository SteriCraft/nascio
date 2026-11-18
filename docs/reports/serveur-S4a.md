# Rapport S4a — Client sans interface : rendu identique par le réseau (2026-11-18)

S4 est découpé en deux. **S4a** (ce rapport) : tout ce dont le client a besoin pour mailler, sans moteur graphique, avec un test d'égalité exacte. **S4b** : un mode réseau du visualiseur (Bevy), que l'auteur vérifiera à l'écran.

## Fait
- **Protocole, version 2** : une colonne transporte ses données de rendu, `ChunkTint` (bloc teinté, couleurs linéaires des quatre coins en `f32` exacts, profondeur de l'eau de chaque colonne locale). Elles viennent du climat du monde, que le client n'a pas. Le serveur les calcule (`chunk_tint` + `water_depths`) en construisant la colonne. Coût : 49 octets plus 1 024 octets de profondeurs, que zstd réduit presque à rien (surtout des 255).
- **`Mirror::padded_chunk`** (`nascio-client`) : assemble un chunk et sa bordure d'un bloc à partir de la colonne et de ses huit voisines (`can_mesh`). Au-dessus d'une colonne : de l'air. Sous ses chunks : chaque colonne locale répète son bloc le plus bas, ce qui cache les faces exactement comme la roche réelle (le générateur commence deux chunks sous la surface la plus basse).
- **Test `crates/server/tests/mesh.rs`** : monde 1/25, serveur réel, client réel. Pour chaque colonne dont les voisines sont reçues, la teinte reçue est égale à celle du terrain local et `mesh_chunk` donne **exactement** le même maillage (positions, normales, couleurs, matériaux) par le réseau et par la génération locale. Résultats : coin du tore (traverse les deux coutures) : 45 colonnes, 45 chunks, 8 662 quads identiques ; second site : 45 colonnes, 50 chunks, 9 101 quads identiques.
- Fin normale d'une connexion (fermeture locale ou par le client) : plus de faux message d'erreur.

## Décisions (détail dans `decisions.md`)
- Les données de rendu par colonne (teinte, profondeur de l'eau) sont calculées par le serveur et envoyées avec la colonne ; `PROTOCOL_VERSION` passe à 2.
- Sous la plage d'une colonne, le client répète le bloc le plus bas.

## Non fait, à décider par l'auteur (invariant 5)
Le client 3D a besoin d'autres données issues du monde : **le ciel et la météo** (nébulosité, températures, neige saisonnière) et **l'horizon** (hauteurs sur des dizaines de kilomètres). Deux voies :
1. Le serveur envoie des **tuiles précalculées** : hauteurs de l'horizon par niveau de détail, cartes climatiques réduites. Le client ne calcule rien ; c'est la voie la plus sûre pour l'invariant, mais elle demande de nouveaux messages et un cache.
2. Le client télécharge un **sous-ensemble du monde macro** (climat, altitude fine) et calcule lui-même le ciel et l'horizon. C'est moins de code, mais le client détient alors une partie des données de génération : la fonction de surface de l'horizon est celle des chunks.

Je recommande la voie 1 pour l'horizon et un sous-ensemble climatique réduit pour le ciel (aucune donnée de géologie ni de relief fin). C'est un choix qui touche à l'invariant 5 : je ne l'ai pas tranché.

## Suite (S4b)
Mode `--connect <adresse> --cert <fichier>` du visualiseur : les colonnes viennent de `nascio-client` et sont maillées avec `Mirror::padded_chunk`, sans génération locale des chunks. Le ciel et l'horizon restent ceux du monde local, autorisé puisque le visualiseur est un outil de développement, en attendant la décision ci-dessus.
