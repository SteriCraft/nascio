# Rapport S4b — Mode réseau du visualiseur (2026-11-18)

## Fait
- **`--connect <adresse> [--cert <fichier>]`** (`crates/viewer/src/remote.rs`) : le certificat est par défaut `<monde>/server/cert.der`. Un fil réseau fait tourner `nascio-client` sur son propre runtime tokio et remplit un `Mirror`. Il vérifie que la table des blocs du serveur est celle du monde local et refuse sinon.
- **Vue 3D** : le `Streamer` ne génère plus de chunks. Il maille une colonne à partir du miroir (`Mirror::padded_chunk` et la teinte reçue) dès que ses huit voisines sont arrivées. L'arrivée d'une colonne ou d'un lot d'éditions (`Blocks`) remet en file la colonne et ses voisines. La caméra envoie `Move` quatre fois par seconde, et c'est cette position que le serveur utilise comme centre de l'envoi des colonnes.
- **Mode dieu** : une édition devient `ClientMsg::SetBlock`. Le bloc change à l'écran quand le serveur la renvoie, et un refus du serveur s'affiche dans le panneau. Les deltas locaux restent en mémoire : en mode réseau, rien n'est écrit dans le monde local.
- La distance de vue est limitée au rayon du serveur moins 64 m, parce qu'une colonne du bord n'a pas toutes ses voisines.
- Le titre de la fenêtre indique le serveur.

## Mesures (monde de référence, `out/m7/world-42`, serveur local)
- Le serveur a envoyé 797 colonnes (0,4 Mo) au visualiseur placé en (50 000, 50 000), en mer près d'une côte.
- Toutes les colonnes visibles ont été maillées depuis le réseau : avec une distance de vue de 448 m, 613 prêtes et 0 manquante. Capture : `out/s4b-net.png`.
- L'image est cohérente : la mer, le fond, la côte, l'horizon et le ciel sont en place.
- Frame à 13,3 ms (vsync 75 Hz).
- Premier essai à 480 m : 36 colonnes du bord ne pouvaient pas être maillées, faute de voisines. D'où la limite à rayon − 64 m.

## Décisions (détail dans `decisions.md`)
- Le réseau du visualiseur tourne dans un fil à part, avec son runtime tokio. Il partage le `Mirror` derrière un mutex, et Bevy n'attend jamais le réseau.
- En mode réseau, une édition n'est affichée qu'une fois confirmée par le serveur. Il n'y a pas de prédiction locale.
- La distance de vue est limitée à `chunk_radius_m − 64`.

## À vérifier par l'auteur
Lancer le serveur puis le visualiseur :

```powershell
cargo run -p nascio-server --release -- out/m7/world-42 --data out/s3-data --bind 127.0.0.1:7777
cargo run -p nascio-viewer --release -- out/m7/world-42 --fly --connect 127.0.0.1:7777 --cert out/s3-data/server/cert.der
```

Points à regarder : voler pour vérifier que les colonnes suivent la caméra ; poser et casser des blocs en mode dieu ; ouvrir deux visualiseurs pour voir une édition apparaître dans l'autre.

## Questions
- Faut-il afficher une édition tout de suite (prédiction locale), puis la corriger si le serveur la refuse ? C'est un choix de ressenti de jeu, que je n'ai pas tranché. Aujourd'hui, l'édition attend l'aller-retour, de l'ordre de 1 à 2 ticks plus le RTT.
- La question du ciel et de l'horizon du client autonome (invariant 5, rapport S4a) reste ouverte. Le visualiseur les prend dans le monde local.
