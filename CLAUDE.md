# CLAUDE.md — Nascio

Ce fichier est lu automatiquement par Claude Code à chaque session. Il rassemble les conventions et les invariants du projet.

## Le projet en bref
Nascio est un jeu multijoueur en voxels de 1 m, sur un monde **torique** de 100 km × 100 km. Le monde est généré de façon procédurale à partir d'une simulation tectonique, d'érosion et de climat. La phase actuelle porte **uniquement sur la création du monde** : génération, géologie, climat, ciel et visualiseur. Le gameplay, le serveur réseau et le site web viendront plus tard.

Documents de référence, à lire avant toute tâche importante :
- `docs/vision.md` : l'intention du projet ;
- `docs/world-spec.md` : la spécification technique du monde, **qui fait autorité** ;
- `docs/roadmap.md` : les jalons et leurs critères de validation ;
- `docs/decisions.md` : le journal des décisions, à compléter à chaque décision structurante.

## Langues
- Code, identifiants, noms de crates, messages de commit : **anglais**.
- Documentation (`docs/`), commentaires de conception, échanges avec l'auteur : **français**.
- Les commentaires de code courts peuvent être en anglais.

## Environnement
- Windows 11, PowerShell, VS Code. Carte graphique RTX 5060 (wgpu passe par Vulkan ou DX12).
- Rust stable avec rustup et la chaîne d'outils MSVC (Visual Studio Build Tools, charge de travail « Développement Desktop en C++ »).
- Les commandes du shell doivent être compatibles PowerShell.

## Architecture (workspace Cargo)
```
crates/
  core/       nascio-core      maths du tore, coordonnées, constantes, temps et astronomie
  world/      nascio-world     données d'un monde lues par la génération des chunks : config,
                               grilles, bruit, fichiers, blocs, codes des couches, gisements
  worldgen/   nascio-worldgen  simulation macro (tectonique, érosion, climat, géologie) + CLI
  chunk/      nascio-chunk     types de chunk sans génération : table des blocs, chunks 32³,
                               teinte, maillage (ce que le client lit)
  voxel/      nascio-voxel     génération de chunks (terrain, géologie, végétation), deltas
  protocol/   nascio-protocol  messages, codec de colonnes v2, trames, entités (sans réseau)
  server/     nascio-server    serveur QUIC : simulation 20 Hz, colonnes, éditions, comptes
  client/     nascio-client    connexion au serveur, miroir des colonnes, entités
  tools/      nascio-tools     outils de développement : chunkbench, topview, geology, coupes
  viewer/     nascio-viewer    visualiseur Bevy : cartes 2D + vue 3D en vol libre
docs/
out/          sorties générées (PNG, mondes de test), ignorées par git
```
Plus tard : `web`.

Sens des dépendances : `core` ← `chunk` ← `world` ← `worldgen` et `voxel` ; `protocol` et `client` ne dépendent que de `core` et `chunk` ; `server` dépend de `voxel` et `protocol` ; `tools` et `viewer` dépendent de tout. `voxel` ne dépend pas de `worldgen` (sauf pour ses tests). **`core` ne dépend d'aucun moteur graphique.** **`client` ne dépend d'aucune crate de génération** (`world`, `worldgen`, `voxel`), ce que vérifie `crates/client/tests/no_generation.rs`.

## Invariants à ne jamais violer
1. **Le tore partout.** Les coordonnées horizontales (x, z) sont périodiques, de période `WORLD_SIZE = 100_000`. Toute différence, distance, voisinage ou interpolation passe par les fonctions de `nascio-core::torus` (`wrap`, `delta`, `distance`…). On ne soustrait **jamais** deux coordonnées brutes. Les tests doivent couvrir les cas qui traversent la couture.
2. **Des coordonnées entières pour les blocs** (`i32`). Les positions continues sont soit un bloc plus un décalage local `f32`, soit des `f64` de monde. Le rendu se fait toujours **relativement à la caméra**.
3. **Le déterminisme.** Une même graine avec la même configuration produit exactement le même monde. On utilise un RNG explicite (`rand_chacha`) dérivé de la graine et d'une étiquette par étape. Interdit : l'itération sur un `HashMap` qui influence un résultat, et le parallélisme dont l'ordre change le résultat.
4. **Bruit périodique.** Tout bruit procédural doit boucler sur 100 000 m : une période de réseau qui divise `WORLD_SIZE`, avec un hachage des nœuds modulo la période.
5. **Le serveur est la seule source de génération.** Le client ne générera jamais de terrain : il dépend de `nascio-chunk` et jamais des crates de génération. Le visualiseur génère localement, mais uniquement en tant qu'outil de développement.
6. **Le temps du jeu est dérivé de l'heure UTC réelle** (voir la spec). Il n'existe aucune horloge interne qui dérive.

## Méthode de travail
- Avancer **jalon par jalon** (`docs/roadmap.md`). S'arrêter à la fin de chaque jalon pour une revue avec l'auteur.
- Pour toute tâche non triviale : proposer d'abord un plan court, puis implémenter.
- **Vérifier visuellement.** Chaque étape de génération exporte des PNG dans `out/`, à basse résolution pour aller vite. Ouvre-les et regarde-les (tu peux lire des images) avant de déclarer une étape terminée. Décris ce que tu vois et ce qui est incohérent.
- Les tests unitaires sont obligatoires pour `core` (tore, temps, astronomie). On ajoute aussi des tests de propriétés (`proptest`) pour la couture et un test de déterminisme qui compare les hachages d'un petit monde généré deux fois.
- Avant de rendre la main : `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`.
- Toute décision structurante va dans `docs/decisions.md` (date, décision, raison). Si la spec doit changer, propose la modification à l'auteur au lieu de diverger en silence.
- Garde tous les paramètres ajustables (tailles de bandes, constantes climatiques…) dans des structures de configuration sérialisables (TOML), plutôt qu'en dur dans le code.

## Commandes usuelles
```powershell
cargo build --workspace
cargo test --workspace
cargo run -p nascio-worldgen --release -- generate --seed 42 --out out/world-42
cargo run -p nascio-viewer --release -- out/world-42
```

## Profils de compilation (Bevy)
- `[profile.dev] opt-level = 1` et `[profile.dev.package."*"] opt-level = 3`.
- La fonctionnalité `bevy/dynamic_linking` est activée en développement uniquement, pour accélérer la compilation.
- La version de Bevy et des crates associées (`bevy_egui`…) est **figée** (`=x.y.z`). Une mise à jour est une décision explicite, consignée dans `docs/decisions.md`.

## Licences
- Le code est sous la licence du fichier `LICENSE`.
- Chaque ressource tierce (textures, sons) est listée dans `ASSETS.md` avec sa source et sa licence. On privilégie CC0.
