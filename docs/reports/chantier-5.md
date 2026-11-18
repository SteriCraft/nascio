# Chantier 5 — Mouvement autoritaire, prédiction et réconciliation

Plan validé par l'auteur le 2026-11-18, avec ses réponses (`decisions.md`) : portée d'édition 5 m, course sur Maj, pas de montée automatique d'un bloc (saut automatique en option), vol sans traverser les blocs, mort par étouffement si un bloc solide est au-dessus de la tête. Étapes : 5.1 physique commune, 5.2 protocole v9, 5.3 serveur, 5.4 prédiction du client, 5.5 visualiseur, 5.6 tests d'intégration et documentation.

## 5.1 — Crate `nascio-physics`
- `PlayerParams` (TOML `[player]`, valeurs de l'auteur, `validate`), `Input` (touches, cap sur 16 bits, vitesse de vol), `State` (pieds, vitesse, au sol, dans un fluide, mode marche ou vol), `Shapes` (forme de collision par identifiant de bloc, tirée de la matière : plein, couche de neige de 0,125 m, fluide, vide), trait `BlockSource` (`None` = colonne non chargée).
- `step` : un pas de 1/60 s. Accélération horizontale vers la vitesse voulue (50 m/s² au sol et dans l'eau, 10 en l'air) ; saut, gravité, chute plafonnée ; nage (monter, plonger, couler à 1 m/s) ; vol sans gravité, plafonné à 120 m/s ; collision axe par axe ; marche de 0,5 m ; bond hors de l'eau contre une berge. Cellule inconnue : état inchangé, `Unloaded`.
- `unstuck` : remet un joueur enfermé sur les blocs qu'il chevauche, ou rend `Suffocated` si la rangée juste au-dessus de sa tête est solide (ou s'il resterait coincé).
- `auto_jump` : pour l'option du client ; vrai au sol devant une marche d'un bloc avec de la place au-dessus.
- 18 tests : chute et atterrissage exact, chute plafonnée, vitesses (marche, course, accroupi), arrêt contre un mur à sa face, bloc non franchi sans saut puis franchi en sautant, neige montée sans saut, hauteur du saut (1,25 m ± 0,1), plafond, nage, vol (collision, plafond de vitesse, descente jusqu'au sol), colonne non chargée, couture (mur de l'autre côté, passage est et ouest), poussée et étouffement, saut automatique, paramètres en TOML ; proptest : le même parcours au bord de la couture et 30 km plus loin donne le même mouvement (à 10⁻⁶ m) ; empreinte figée d'une trajectoire de 2 000 pas (même valeur en debug et en release ; à vérifier sous Linux).

Limites connues : la hauteur ne change pas accroupi, et l'on peut tomber d'un bord accroupi (pas de garde au bord) ; ~~le magma est un fluide où l'on nage~~ (erreur, corrigée en 5.3 : le magma est un bloc solide, on marche dessus ; sa mort au contact est faite en 5.3).

## 5.2 — Protocole v9
Plan validé par l'auteur (octet de type, `f64` exacts, 32 commandes, mouvement réseau figé jusqu'à 5.3).

- **Datagrammes typés** : un octet en tête. Serveur → client : `0` entités (format S5 inchangé derrière l'octet), `1` `PlayerState`. Client → serveur : `0` commandes.
- **`Commands`** (`crates/protocol/src/player.rs`) : type, n° de la commande la plus récente (varint), nombre (1 à 32), tangage (u8, pour la réplication), puis les commandes de la plus ancienne à la plus récente. La première est entière (touches, cap u16 LE, vitesse de vol en varint), les suivantes font un octet de touches ; le bit 7 annonce un varint `zigzag(Δcap) << 1 | vitesse de vol changée`, suivi de la nouvelle vitesse si elle a changé. Décodage strict (type, nombre, n° ≥ nombre − 1, bit 7 interdit en tête, changement vide, octets en trop).
- **`PlayerState`** : type, tick (varint), `ack + 1` (varint, 0 = aucune commande), drapeaux (au sol, dans un fluide, vol, masque des composantes de vitesse non nulles), position en trois `f64` LE, puis les composantes de vitesse non nulles (−0,0 est envoyé). Refus à l'encodage d'un état non fini ; au décodage, d'une valeur non finie, d'une position hors du tore, de drapeaux inconnus.
- **`InputQueue`** : la file du serveur (ordre, pas de doublon, trous sautés et comptés, file pleine sans perte).
- **Messages** : `ServerHello.physics` (`PlayerParams`) et `ServerHello.mode` ; `Move`, `Correct` et `max_speed_m_s` retirés ; `[player]` dans la configuration du serveur, validé au démarrage ; `input_datagrams_per_s` et `input_datagrams_burst` (40).
- **Tailles mesurées** (test `player::tests::sizes`, charge utile ; QUIC, UDP et IP ajoutent ≈ 55 o) :

| Datagramme | Octets |
|---|---|
| 9 commandes, au repos (une heure de jeu) | 18 |
| 9 commandes, en tournant à 90°/s | 34 |
| 32 commandes, pire cas réaliste | 165 (borne 231) |
| `PlayerState` au repos / en marche / en saut | 32 / 48 / 56 |

  Soit 1,5 à 1,8 Ko/s en montée et 1,7 à 2,2 Ko/s en descente par joueur, en-têtes compris.
- **Tests** (10 nouveaux, 29 dans `nascio-protocol`) : aller-retour des commandes (cap à travers 0, vitesse de vol, 32 commandes, toute troncature refusée) et de l'état au bit près (−0,0, valeurs extrêmes) ; refus ; file : pertes de 1 à 9 datagrammes d'affilée sans perte de commande, 11 perdus = trou compté, doublons et datagrammes en retard appliqués une fois ; proptest d'aller-retour et de pertes, doublons et retards quelconques (ordre strict, chaque commande appliquée ou comptée sautée).
- **Appelants** :
  - serveur : une tâche lit les datagrammes du client (seau de jetons, décodage, abus consignés `datagram` et `inputs`), remplit `PlayerLink::inputs` et reprend le cap et le tangage de la commande la plus récente pour la réplication. **Le serveur ne simule pas encore : le joueur reste à son point d'apparition (5.3).** La portée d'édition se mesure depuis `View::pos` ;
  - client : `Event::Player`, `Connection::send_commands` ;
  - `netbot --walk` devient un drapeau (marche en cercle depuis le point d'apparition) ;
  - `swarm` marche vers un but errant d'après le dernier `PlayerState` et compte les états reçus ;
  - visualiseur en mode réseau : commandes sans touche portant le cap et le tangage de la caméra.
- **Tests d'intégration** : `entities.rs` est ignoré jusqu'à 5.3 (le joueur ne bouge pas encore) et ses envois sont convertis en commandes ; `security.rs` remplace le test de téléportation par une avalanche de datagrammes de commandes (abus `inputs`).

## 5.3 — Simulation des joueurs par le serveur
Plan validé par l'auteur (protocole v10, crédit de pas, commandes fabriquées après 250 ms, magma mortel au contact, `SetBlock.cmd`, vol par la console et le client).

- **Corps** (`crates/server/src/movement.rs`) : état physique, crédit de pas (+3 par tick, 9 au plus, 4 pas par tick au plus), ticks sans commande ni datagramme, mort, installation à faire (`unstuck` à l'arrivée et à la réapparition), dernière commande, yeux des 64 dernières commandes.
  - Une commande dont une cellule est inconnue reste en file : le joueur attend, puis la rejoue telle quelle.
  - Après 5 ticks sans commande en file ni datagramme reçu (et après la première commande), des pas sans touche, numérotés à la suite (`InputQueue::fill`) et acquittés ; les vraies commandes de ces numéros sont ignorées ensuite.
  - Toucher du magma (1 cm) tue ; un mort vide sa file sans bouger (commandes acquittées).
- **Blocs de la simulation** (`world_view.rs`) : colonnes décodées des octets envoyés aux clients, lues comme `Mirror` (air au-dessus, inconnu au-dessous) ; construites par une tâche tokio (le tick n'attend jamais) ; demandées 2,5 m autour de chaque joueur à chaque tick ; éditions appliquées sur place ; `sim_columns` = 2 048 au plus (jamais une colonne utilisée au tick précédent).
- **Tick** (`sim.rs`) : entrées, puis éditions prêtes (ordre des joueurs), diffusion des blocs, mouvement de chaque corps (ordre des joueurs), puis pour chaque client son `PlayerState` et les entités, dans le même budget. `View::pos` vient de la simulation ; la tâche des commandes ne garde que le cap et le tangage. Les morts ne sont pas répliqués.
- **Éditions** : contrôlées dans la simulation. Portée de 5 m des yeux après la commande `cmd` au point le plus proche de la cellule ; une édition qui précède sa commande attend une seconde au plus ; pose d'un bloc qui fait obstacle refusée dans un joueur vivant (« inside a player ») ; refusée à un mort ; `unstuck` sur un joueur qu'une édition enfermerait (en pratique, la pose est refusée avant).
- **Protocole v10** : `Died { cause }` (`Suffocation`, `Magma`), `Respawned { pos }`, `Respawn`, `Fly(bool)`, `SetBlock.cmd`. `InputQueue::peek`, `fill`, `started`.
- **Vol** : `Fly` accepté d'un administrateur (vérifié à chaque demande, refus consigné `fly`), commande de console `fly <nom> on|off` ; marche à chaque connexion.
- **Configuration** : `edit_reach_m` vaut 5 (mesurée des yeux) ; `sim_columns` ajouté. Statistiques : pas (avec et sans commandes), morts, colonnes décodées et leurs octets, durée du mouvement (console `stats` et ligne toutes les 10 s).
- **Tests** :
  - unitaires (`movement.rs`, 5) : crédit et rattrapage, famine (figé 250 ms puis chute, acquittement des pas fabriqués, commandes tardives ignorées), cellules inconnues, magma, mort et réapparition, yeux et portée à travers la couture ; `InputQueue::fill` (protocole) ;
  - `tests/movement.rs` : 120 datagrammes (360 commandes : marche, course en tournant, sauts, recul, accroupi), 30 perdus (jusqu'à 3 d'affilée), 14 dupliqués, 10 en retard → **104 états du serveur égaux au bit près** à une simulation locale de `nascio-physics` sur les blocs reçus par le client ; aucun pas fabriqué ;
  - `tests/entities.rs` réactivé : le joueur court à 5,6 m/s en tournant et en sautant ; l'observateur (150 ms, gigue 30 ms, 5 % de pertes) le voit à 9 mm en moyenne du mouvement autoritaire (15 cm au pire), vitesse mesurée 5,60 m/s ;
  - `tests/rules.rs` : pose dans un joueur et hors de portée refusées ; arrivée dans deux blocs → posé dessus ; dans trois blocs → monté sur le dessus ; vol refusé à un joueur, accordé à un administrateur, retiré par la console ; `security.rs` : le refus de portée vient de la simulation (journal `reach`) ;
  - la mort (étouffement, magma), le refus d'édition au mort et la réapparition sont couverts par les tests unitaires de `movement` (aucune voie réseau ne peut plus enfermer un joueur : la pose est refusée).
- **Corrections trouvées par la mesure à 500 bots** (première passe) :
  - 17 comptes sur 500 mouraient à l'arrivée : `spawn::standing` pose le joueur sur le relief généré, sans les arbres, et certains points tombent dans un tronc (mort à chaque réapparition). À l'arrivée et à la réapparition, un joueur enfermé monte désormais à la première place libre (64 m au plus) ; l'étouffement reste pour l'enfermement par une édition. **À confirmer par l'auteur** (`decisions.md`).
  - Après une famine pendant la montée en charge, plus aucune commande des bots n'était appliquée (1,4 M pas fabriqués) : les datagrammes portaient des numéros déjà comblés, la file restait vide et le serveur continuait de combler, au rythme même du client. Correction : la famine se compte en ticks **sans aucun datagramme** (`InputQueue::pushes`), pas en ticks à file vide. Règle ajoutée en plus : un client qui reçoit un acquittement au-delà de sa dernière commande numérote la suivante après lui (swarm, netbot ; le client en 5.4), pour reprendre sans perdre l'écart.

**Mesure à 500 bots** (`swarm`, serveur et bots sur la même machine de 12 fils, `swarm` en priorité basse, `out/server/world-42`, chaque compte à son point d'apparition). Dernière passe, avec 500 joueurs connectés :

| | Tick p50 | Tick p99 | Mouvement p99 | Réplication p99 | Colonnes décodées |
|---|---|---|---|---|---|
| 500 joueurs | 3,8 à 7,4 ms | 25 à 34 ms | 2,8 à 4,7 ms | 18 à 31 ms | 626 colonnes, 128 Mo |

- Le mouvement de 500 corps (60 pas par seconde chacun, 30 000 pas/s) coûte **3 à 5 ms au p99** sur un seul fil : pas besoin de le répartir sur le pool (seuil prévu de 5 ms). Le p99 du tick reste dominé par la réplication (inchangée), dans le budget de 50 ms.
- Les colonnes décodées pour la simulation : 626 pour 500 joueurs répartis sur 10 sites, 128 Mo (≈ 200 Ko par colonne) ; le plafond de 2 048 colonnes tient ≈ 420 Mo.
- **Limite de la mesure** : `swarm` sur la même machine ne tient pas ses 20 envois par seconde (3 datagrammes de commandes par seconde et par bot en moyenne, trous jusqu'à 1,5 s ; il décode 360 000 colonnes) : le serveur comble les trous par des pas sans touche, comme prévu, et la plupart des pas sont fabriqués. Le coût mesuré reste le bon (un pas fabriqué fait la même physique), mais le chemin des vraies commandes à 500 joueurs se vérifiera avec les bots sur une autre machine (comme pour S6). Avec 10 bots, tous les pas viennent des commandes (600 pas/s, aucun fabriqué).
- Aucune mort, aucun abus sinon quelques refus de portée (bots affamés, qui visent depuis une position périmée).


Limites connues :
- Le client (`Mirror`) ajoute de l'air quand une édition tombe sous la plage d'une colonne, alors que le serveur redemande la colonne : prédiction fausse jusqu'au renvoi (creuser plus de deux chunks sous la surface la plus basse) ; à corriger en 5.4.
- Le visualiseur envoie `cmd: None` jusqu'en 5.5 : ses éditions sont mesurées depuis la position présente.
- Le passage marche ↔ vol n'est pas numéroté avec les commandes : une petite correction côté client au basculement.

Texte proposé pour la spec (§8.1, §8.4 et §8.5), mis à jour en 5.3, à intégrer en 5.6 :
- §8.1 : `PROTOCOL_VERSION` vaut 10 ; datagrammes non fiables pour les états des entités, les commandes du joueur et son état autoritaire, chacun précédé d'un octet de type. Le client dépend aussi de `nascio-physics`.
- §8.4 (nouveau point, **le mouvement**) : le client envoie à 20 Hz ses commandes numérotées, une par pas de 1/60 s, en répétant les 32 dernières non acquittées. Le serveur simule chaque joueur avec `nascio-physics`, contre les blocs mêmes qu'il envoie aux clients (air au-dessus d'une colonne, inconnu au-dessous) : chaque tick, au plus 3 pas plus un de rattrapage, dans un crédit de 3 pas par tick gardé jusqu'à 9 ; une commande dont une cellule est inconnue attend. Sans aucun datagramme de commandes pendant 250 ms, le serveur avance le joueur sans touche, numérote ces pas à la suite et les acquitte (les vraies commandes de ces numéros sont ignorées ; un client qui reçoit un acquittement au-delà de sa dernière commande numérote la suivante après l'acquittement). Il renvoie à chaque tick, avant les entités et dans le même budget, l'état du joueur (position et vitesse exactes, n° de la dernière commande appliquée), sur lequel le client rejoue ses commandes suivantes.
- §8.4 (suite, **mort et vol**) : un joueur enfermé à l'arrivée, à la réapparition ou par une édition est posé sur les blocs, sauf si la rangée au-dessus de sa tête est solide : il meurt d'étouffement. Toucher le magma tue. `Died { cause }` prévient le client ; un mort ne bouge plus et n'est plus vu des autres ; `Respawn` le ramène au point de son compte (`Respawned`). Le vol est réservé aux administrateurs (`Fly`, ou la commande de console `fly`), sans gravité, sans traverser les blocs.
- §8.5 : « les positions (`Move`) et » disparaît ; le point « Un déplacement plus rapide… (`Correct`) » est remplacé par : le serveur simule lui-même le joueur, et un seau de jetons limite les datagrammes de commandes (`input_datagrams_per_s`). La portée d'édition (`edit_reach_m`, 5 m) se mesure des yeux, tels qu'ils étaient après la commande que porte `SetBlock.cmd`, au point le plus proche de la cellule ; une édition qui précède sa commande attend au plus une seconde ; un bloc qui fait obstacle ne se pose pas dans un joueur.
