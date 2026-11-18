# Rapport S5 — Entités (2026-11-18)

## Fait
- **Format des datagrammes d'entités** (`nascio-protocol::entity`, protocole v3) :
  - chaque datagramme a son propre numéro (`seq`) et le tick du serveur ;
  - une entité envoyée en différence nomme le datagramme qui lui sert de base (`seq − base`, un varint), un datagramme que le client a acquitté ;
  - nouveau message `ClientMsg::Ack { seqs }`, sur le flux de contrôle ;
  - une base n'a jamais plus de 64 datagrammes (`BASELINE_WINDOW`).
- **Réplication côté serveur** (`crates/server/src/replicate.rs`), à la fin de chaque tick, dans le fil de simulation :
  - une grille spatiale de 32 m ;
  - pour chaque client, un accumulateur de priorité par entité, dont le poids est `1 / (1 + (d/32)²)` ;
  - les 64 entités de plus haute priorité partent à chaque tick, et toutes les entités dans le rayon partent au moins tous les 4 ticks (5 Hz) ;
  - les datagrammes font au plus 1 200 octets (ou ce que permet le chemin QUIC) ;
  - une entité sortie du rayon est envoyée comme retirée tant que ce retrait n'est pas acquitté ;
  - les octets des entités sont décomptés du même budget par client que les colonnes, et passent en premier.
  - Réglages TOML : `entity_radius_m` (256), `entity_fast` (64), `entity_min_hz` (5), `max_datagram_bytes` (1 200).
- **Client** (`nascio-client::Entities`) :
  - décodage contre l'historique des datagrammes reçus ;
  - une piste d'échantillons par entité ;
  - correspondance entre heure locale et tick, prise sur les datagrammes les plus rapides des 2 dernières secondes et lissée ;
  - affichage 100 ms derrière le serveur, avec interpolation linéaire sur le tore (`delta_q`) ;
  - extrapolation sur au plus 250 ms, puis arrêt ;
  - un retrait plus ancien qu'un état reçu est ignoré.
- **Visualiseur** (`--connect`) :
  - les autres joueurs sont des pavés de 0,6 × 1,8 × 0,6 m, placés relativement à l'origine de rendu ;
  - la caméra est envoyée à 20 Hz ;
  - capture : `out/s5-players.png`, trois bots en cercle devant la caméra.
- **`netbot --walk x,y,z --hold <s>`** : les bots tournent en cercle autour d'un point.

## Mesures
- **Test `crates/server/tests/entities.rs`** :
  - conditions : serveur réel ; un joueur tourne sur un cercle de 10 m qui traverse les deux coutures ; l'observateur reçoit avec **150 ms de latence dans chaque sens**, 30 ms de gigue et 5 % de datagrammes perdus ;
  - résultat : écart au cercle de 1,4 cm au plus ; vitesse angulaire moyenne de 0,501 rad/s pour 0,500 attendu ; sur des fenêtres de 50 ms, entre 0,42 et 0,65 rad/s ; aucun trou, aucun retour en arrière.
- **Ce qui reste irrégulier** : le joueur envoie sa position toutes les 10 ms, et le serveur l'échantillonne à son tick. L'instant réel de chaque échantillon varie donc de ±10 ms, d'où une irrégularité de ±25 % sur 50 ms.
- **Débit** : une entité en différence coûte de 7 à 10 octets, et environ 14 en état complet. Dans le test, l'observateur a reçu 178 o/s pour une entité à 20 Hz, en-têtes compris.
- **Test unitaire de foule** (300 entités autour d'un client, à cheval sur la couture, 10 % de pertes) :
  - aucune entité n'attend plus de 4 ticks ;
  - les 3 plus proches partent à chaque tick ;
  - aucun datagramme ne dépasse 1 200 octets.
- **Serveur avec 3 bots et le visualiseur** : tick au p99 de 0,24 ms.

## Décisions (détail dans `decisions.md`)
- Numéro par datagramme et base par entité, plutôt qu'une base par instantané : une entité choisie par priorité n'est pas dans chaque instantané, et les datagrammes arrivent dans le désordre.
- Acquittements sur le flux de contrôle fiable, plutôt que dans des datagrammes.
- Réplication dans le fil de simulation, triée par identifiant : déterministe. Le coût avec 500 joueurs sera mesuré en S6, et parallélisé par client si besoin.
- Mouvement du joueur décidé par le client (position envoyée par `Move`). Le serveur le réplique sans le simuler.

## Non décidé (gameplay), noté
- **Autorité sur le mouvement et collision.** Le roadmap demande une « collision minimale » et une prédiction. Un mouvement simulé par le serveur, avec prédiction et réconciliation côté client, demande une physique du joueur : gravité, vitesse de marche, de saut et de vol, dimensions, comportement dans l'eau. Ce sont des choix de gameplay, que je n'ai pas faits. Aujourd'hui, le client se déplace librement ; sa propre « prédiction » est donc son état local, sans correction. En S7, le serveur pourra borner la vitesse et refuser une position dans un bloc plein, avec des valeurs à fixer par l'auteur.
- La forme et la taille de l'avatar : le pavé de 1,8 m est un repère de développement.

## Questions
1. Quel modèle de mouvement veux-tu ? (a) Le client fait autorité et le serveur vérifie des bornes, ce qui est simple et suffisant pour du vol libre. (b) Le serveur fait autorité, avec prédiction et réconciliation, ce qui est nécessaire pour des combats équitables. Je recommande (b) dès que la physique du joueur sera définie.
2. Faut-il un rayon de réplication plus grand que 256 m, par exemple pour voir une armée arriver de loin ? Au-delà, il faudrait un niveau de détail plus grossier (positions à 1 Hz, sans orientation).
