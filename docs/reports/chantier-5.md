# Chantier 5 — Mouvement autoritaire, prédiction et réconciliation

Plan validé par l'auteur le 2026-11-18, avec ses réponses (`decisions.md`) : portée d'édition 5 m, course sur Maj, pas de montée automatique d'un bloc (saut automatique en option), vol sans traverser les blocs, mort par étouffement si un bloc solide est au-dessus de la tête. Étapes : 5.1 physique commune, 5.2 protocole v9, 5.3 serveur, 5.4 prédiction du client, 5.5 visualiseur, 5.6 tests d'intégration et documentation.

## 5.1 — Crate `nascio-physics`
- `PlayerParams` (TOML `[player]`, valeurs de l'auteur, `validate`), `Input` (touches, cap sur 16 bits, vitesse de vol), `State` (pieds, vitesse, au sol, dans un fluide, mode marche ou vol), `Shapes` (forme de collision par identifiant de bloc, tirée de la matière : plein, couche de neige de 0,125 m, fluide, vide), trait `BlockSource` (`None` = colonne non chargée).
- `step` : un pas de 1/60 s. Accélération horizontale vers la vitesse voulue (50 m/s² au sol et dans l'eau, 10 en l'air) ; saut, gravité, chute plafonnée ; nage (monter, plonger, couler à 1 m/s) ; vol sans gravité, plafonné à 120 m/s ; collision axe par axe ; marche de 0,5 m ; bond hors de l'eau contre une berge. Cellule inconnue : état inchangé, `Unloaded`.
- `unstuck` : remet un joueur enfermé sur les blocs qu'il chevauche, ou rend `Suffocated` si la rangée juste au-dessus de sa tête est solide (ou s'il resterait coincé).
- `auto_jump` : pour l'option du client ; vrai au sol devant une marche d'un bloc avec de la place au-dessus.
- 18 tests : chute et atterrissage exact, chute plafonnée, vitesses (marche, course, accroupi), arrêt contre un mur à sa face, bloc non franchi sans saut puis franchi en sautant, neige montée sans saut, hauteur du saut (1,25 m ± 0,1), plafond, nage, vol (collision, plafond de vitesse, descente jusqu'au sol), colonne non chargée, couture (mur de l'autre côté, passage est et ouest), poussée et étouffement, saut automatique, paramètres en TOML ; proptest : le même parcours au bord de la couture et 30 km plus loin donne le même mouvement (à 10⁻⁶ m) ; empreinte figée d'une trajectoire de 2 000 pas (même valeur en debug et en release ; à vérifier sous Linux).

Limites connues : la hauteur ne change pas accroupi, et l'on peut tomber d'un bord accroupi (pas de garde au bord) ; le magma est un fluide où l'on nage, alors que la spec le dit mortel (à décider avec la mort par étouffement en 5.3).

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

Texte proposé pour la spec (§8.1, §8.4 et §8.5), à intégrer en 5.6 avec les règles de 5.3 :
- §8.1 : `PROTOCOL_VERSION` vaut 9 ; datagrammes non fiables pour les états des entités, les commandes du joueur et son état autoritaire, chacun précédé d'un octet de type. Le client dépend aussi de `nascio-physics`.
- §8.4 (nouveau point) : le client envoie à 20 Hz ses commandes numérotées, une par pas de 1/60 s, en répétant les 32 dernières non acquittées ; le serveur les applique dans l'ordre et renvoie à chaque tick l'état du joueur (position et vitesse exactes, n° de la dernière commande appliquée), sur lequel le client rejoue ses commandes suivantes.
- §8.5 : « les positions (`Move`) et » disparaît ; le point « Un déplacement plus rapide… (`Correct`) » est remplacé par : le serveur simule lui-même le joueur, et un seau de jetons limite les datagrammes de commandes (`input_datagrams_per_s`).
