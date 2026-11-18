# S6 sur deux machines — marche à suivre

But : mesurer le serveur seul. Le serveur tourne sur le portable Linux, et `swarm` (500 bots) sur le PC Windows. La première mesure de S6 faisait tourner les deux sur le même PC ; elle a montré une dérive du tick médian, de 7 ms à environ 40 ms au bout d'une minute, qu'on veut confirmer ou écarter.

Dans la suite, `<IP_LINUX>` désigne l'adresse du portable sur le réseau local (par exemple `192.168.1.20`).

## 0. Avant de commencer
- **Réseau** : relie les deux machines **par câble** au même commutateur ou à la même box, si possible en gigabit. À 500 bots, le serveur envoie environ 110 Mbit/s, ce qu'un Wi-Fi tient mal. Si seul le Wi-Fi est possible, fais d'abord la mesure à 250 bots (environ 35 Mbit/s), et indique-le dans le compte rendu.
- **Portable** : branché sur le secteur, mise en veille désactivée, et réglage d'énergie sur « performance ». Sous Linux, si `powerprofilesctl` existe : `powerprofilesctl set performance`.
- **Même version du code** des deux côtés : le commit `d6698a2` ou un plus récent, le même sur les deux machines.

## 1. Linux : compiler le serveur
```bash
# Outils (Debian/Ubuntu ; adapter le gestionnaire de paquets sinon)
sudo apt update
sudo apt install -y build-essential pkg-config git curl sysstat
# Rust (si absent)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"

# Le code
git clone https://github.com/SteriCraft/nascio.git ~/nascio
cd ~/nascio
# Clone déjà existant : le mettre à jour
git checkout main && git pull
# Vérifier : doit afficher d6698a2 (ou un commit plus récent, le même que sur le PC Windows)
git log -1 --oneline

# Seulement le serveur : pas besoin des dépendances de Bevy
cargo build -p nascio-server --release
```
Le binaire est `~/nascio/target/release/nascio-server`.

Depuis le chantier 2, l'inscription est fermée et la connexion par secret est réservée aux administrateurs. `--open-registration` rétablit l'ancien comportement pour la mesure : les bots se connectent par secret et leurs comptes sont créés à la volée. Pour mesurer aussi la connexion par jeton, voir la fin du rapport `chantier-2.md`.

## 2. Le monde à copier
Le serveur a besoin du monde de référence `out/m7/world-42` (364 Mo, sans deltas).

Sur le PC Windows (PowerShell, à la racine du dépôt) :
```powershell
Compress-Archive -Path out\m7\world-42 -DestinationPath out\world-42.zip -Force
scp out\world-42.zip <utilisateur>@<IP_LINUX>:~/
```
Une clé USB convient aussi, si `scp` n'est pas disponible.

Sur Linux :
```bash
mkdir -p ~/nascio-s6
cd ~/nascio-s6
unzip ~/world-42.zip     # crée ~/nascio-s6/world-42
ls world-42/layers | wc -l   # environ 80 fichiers
```
Le dossier de données du serveur (deltas, certificat, comptes, journal des abus) sera `~/nascio-s6/data`. C'est un dossier neuf, donc les comptes des bots seront créés à leur première connexion.

## 3. Réglages système Linux
QUIC passe par UDP. Agrandis les tampons UDP, sinon le noyau perd des paquets en rafale :
```bash
sudo sysctl -w net.core.rmem_max=8388608
sudo sysctl -w net.core.wmem_max=8388608
```
Ces réglages durent jusqu'au prochain redémarrage.

## 4. Pare-feu
Le serveur écoute sur le **port UDP 7777**.

Sur Linux, avec `ufw` :
```bash
sudo ufw allow 7777/udp
sudo ufw status
```
Avec `firewalld` à la place : `sudo firewall-cmd --add-port=7777/udp`.

Sous Windows, aucune règle n'est nécessaire en sortie. Si Windows demande d'autoriser `swarm.exe` au premier lancement, accepte pour les réseaux privés.

Vérifie aussi que le réseau du PC est classé « Privé » : Paramètres → Réseau et Internet → Propriétés.

## 5. Premier lancement du serveur et certificat
Sur Linux :
```bash
cd ~/nascio
./target/release/nascio-server ~/nascio-s6/world-42 --data ~/nascio-s6/data --bind 0.0.0.0:7777 --server-id lab --open-registration
```
Au premier lancement, le serveur crée son certificat dans `~/nascio-s6/data/server/cert.der`.
- Arrête-le avec la commande `stop` (tape-la puis Entrée), ou avec Ctrl-C.
- Copie ce fichier sur le PC Windows. Depuis le PC :

```powershell
scp <utilisateur>@<IP_LINUX>:~/nascio-s6/data/server/cert.der out\s6-linux-cert.der
```
Le certificat est émis pour « localhost », et le client se présente toujours sous ce nom : il est accepté quelle que soit l'adresse IP. Si tu supprimes `data/server`, un nouveau certificat est créé et il faut le recopier.

## 6. Windows : compiler `swarm`
Dans le dépôt, sur le même commit :
```powershell
cargo build -p nascio-tools --release --bin swarm
```

## 7. Essai rapide (50 bots, environ 1 minute)
Sur Linux, relance le serveur avec un journal :
```bash
cd ~/nascio
./target/release/nascio-server ~/nascio-s6/world-42 --data ~/nascio-s6/data --bind 0.0.0.0:7777 --server-id lab --open-registration 2>&1 | tee ~/nascio-s6/server-test.log
```
Sur Windows :
```powershell
.\target\release\swarm.exe <IP_LINUX>:7777 out\s6-linux-cert.der --bots 50 --settle 10 --duration 20
```
Résultat attendu :
- côté `swarm` : `errors 0`, et des « entities seen » entre 30 et 49 environ ;
- côté serveur : `players 50`.

Si les bots ne se connectent pas :
- vérifie l'adresse IP, le pare-feu (§4) et le certificat (§5) ;
- vérifie que les deux machines se voient (`ping <IP_LINUX>`).

Arrête ensuite le serveur avec `stop`.

## 8. La mesure : 500 bots pendant 5 minutes
Le serveur affiche ses statistiques toutes les 10 s. Les percentiles du tick portent sur la **dernière minute** (1 200 ticks).

**Linux, terminal 1 : le serveur.**
```bash
cd ~/nascio
./target/release/nascio-server ~/nascio-s6/world-42 --data ~/nascio-s6/data --bind 0.0.0.0:7777 --server-id lab --open-registration 2>&1 | tee ~/nascio-s6/server-500.log
```

**Linux, terminal 2 : CPU et mémoire du serveur toutes les 10 s.** Lance-le une fois le serveur démarré, avant les bots. Si tu relances le serveur, relance aussi `pidstat` : il suit un numéro de processus.
```bash
pidstat -u -r -h -p "$(pgrep -f target/release/nascio-server)" 10 | tee ~/nascio-s6/pidstat-500.log
```

**Linux, terminal 3 (facultatif) : la charge par cœur, pour voir si un cœur sature.**
```bash
mpstat -P ALL 10 | tee ~/nascio-s6/mpstat-500.log
```

**Windows : les bots.** 500 bots, connexion en 10 s, 20 s de stabilisation, 5 minutes de mesure :
```powershell
.\target\release\swarm.exe <IP_LINUX>:7777 out\s6-linux-cert.der --bots 500 --settle 20 --duration 300 2>&1 | Tee-Object out\s6-swarm-500.log
```
Pendant la mesure, garde le Gestionnaire des tâches ouvert sur le PC (onglet Performance). Note le CPU de `swarm.exe` et l'usage réseau, deux ou trois fois.

Quand `swarm` a fini :
- tape `stats`, puis `stop`, dans le terminal du serveur ;
- arrête `pidstat` et `mpstat` avec Ctrl-C.

**Si tu as le temps : même mesure à 250 bots**, avec `--bots 250` et des fichiers `*-250.log`.

## 9. Mesures à me rapporter
Le plus simple : envoie-moi les fichiers `~/nascio-s6/server-500.log`, `pidstat-500.log`, `mpstat-500.log` (s'il existe) et `out\s6-swarm-500.log`. Ou, à défaut, les éléments suivants :

1. **La dérive du tick médian** : toutes les lignes `players 500 · tick p50 … p99 …` du journal du serveur, du début à la fin. La question est de savoir si le p50 reste stable, autour de 5 à 10 ms, ou s'il saute vers 30 à 40 ms au bout d'une minute environ comme sur une seule machine. Si oui, à quel moment.
2. **Le p99 du tick et le p99 de la réplication** sur la dernière ligne (objectif de S6 : p99 < 50 ms), et le maximum observé.
3. **Les clients sautés** (`skipped`) et les événements d'abus (`abuse events`), sur la dernière ligne. Il ne devrait pas y en avoir.
4. **Les trois dernières lignes de `swarm`** :
   - octets reçus par bot (p50, p99, max ; objectif ≤ 128 Ko/s) ;
   - datagrammes par seconde ;
   - entités vues (min, moyenne) ;
   - erreurs ;
   - éventuels « bots seeing no entity ».
5. **Le CPU du serveur** (`%CPU` dans `pidstat`, 100 % = un cœur) et sa **mémoire** (`RSS`), au début et à la fin.
6. **Le contexte** :
   - modèle du processeur du portable (`lscpu | head -20`) et sa mémoire ;
   - liaison (câble ou Wi-Fi, débit) ;
   - CPU de `swarm.exe` côté PC ;
   - distribution Linux.
7. **Tout message d'erreur** vu d'un côté ou de l'autre.

## 10. Après la mesure
- Le dossier `~/nascio-s6/data` contient les comptes `swarm0` à `swarm499`, les deltas des éditions des bots et `server/abuse.log`. Garde-le jusqu'à ce que j'aie lu les résultats.
- Pour une nouvelle mesure partant de zéro, supprime `~/nascio-s6/data/deltas`. Garde `data/server`, sinon il faut recopier le certificat.
