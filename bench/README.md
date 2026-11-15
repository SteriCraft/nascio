# Benchmark du visualiseur 3D

Mesure reproductible des temps d'image le long d'un trajet de caméra scripté.

## Lancer le benchmark (Linux ou Windows)

```sh
# 1. Générer le monde de référence (graine 42, pleine résolution, ~7 s) :
cargo run -p nascio-worldgen --release -- generate --seed 42 --scale 1 --out out/m2/world-42

# 2. Lancer le trajet par défaut (réglages maximum) :
cargo run -p nascio-viewer --release -- out/m2/world-42 --bench bench/default.toml --bench-out out/bench.csv --fullscreen
```

Sous Linux, les dépendances système de Bevy sont celles de la CI : `libasound2-dev libudev-dev libwayland-dev libxkbcommon-dev`.

Le mode benchmark impose :
- le mode 3D ;
- une fenêtre de 1920×1080 pixels physiques, quelle que soit la mise à l'échelle du bureau ;
- pas de synchronisation verticale ;
- l'horizon actif ;
- la distance de vue du fichier (1 024 m par défaut, le maximum) ;
- une heure solaire fixe ;
- la courbure d'affichage de la configuration de rendu (rayon réel par défaut), sauf si le fichier fixe `curvature_radius_m` (0 : monde plat) ou si `--curvature-radius` est passé.

`--fullscreen` passe en plein écran sans bordure : sur un écran 1080p, c'est exactement du 1920×1080. **Rien ne doit recouvrir la fenêtre** pendant la mesure. Une fenêtre masquée ou sans le focus peut être bridée à 60 i/s par le compositeur, ce qui fausse le résultat.

## Déroulement

1. La caméra attend au premier point de passage que tout soit chargé (chunks et horizon), avec 120 s au plus.
2. Elle suit ensuite le trajet en temps réel, environ 125 s :
   - vol bas à 60 m/s à travers la couture est-ouest, puis la couture nord-sud ;
   - montée et traversée de 38 km à 2 km/s ; le chargement ne suit pas, c'est l'horizon qui couvre ;
   - passage rapide (250 m/s) le long de la plus haute chaîne.
3. Chaque image est enregistrée, puis le programme s'arrête.

## Sorties

- `out/bench.csv` : une ligne par image avec `frame, t_s, frame_ms, cpu_ms, render_cpu_ms, gpu_ms, x, y, z, columns_ready, columns_missing, chunk_quads, horizon_triangles`.
- `out/bench.summary.txt`, également affiché à la fin : adaptateur, résolution, temps d'image (moyenne, p50, p95, p99, max), i/s moyen et « 1 % low », part des images au-delà de 16,7 ms, temps CPU et GPU.
- Le verdict est **PASS** si le p95 du temps d'image reste sous 16,7 ms (60 i/s).

## Signification des temps

- **`frame_ms`** : temps réel entre deux images ; c'est ce que ressent le joueur.
- **`cpu_ms`** : temps du programme principal par image (simulation, chargement, interface).
- **`render_cpu_ms`** : temps CPU d'encodage des passes de rendu.
- **`gpu_ms`** : temps GPU des passes instrumentées par Bevy, mesuré par requêtes d'horodatage (Vulkan, DX12). Quelques passes, comme egui, ne sont pas instrumentées. La colonne est vide si l'adaptateur ne permet pas ces mesures.

## Trajet de captures

`bench/seams.toml` sert aux captures et non à la mesure. Il produit, dans `out/m2/`, des images à travers les deux coutures, au-dessus du coin où elles se croisent, et une vue lointaine de montagnes.
