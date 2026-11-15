# Prompt de démarrage (à coller dans Claude Code)

---

Bonjour. Nous démarrons le projet **Nascio**. Le dépôt ne contient pour l'instant que la documentation.

1. Lis entièrement `CLAUDE.md`, `docs/vision.md`, `docs/world-spec.md`, `docs/roadmap.md` et `docs/decisions.md`.
2. Vérifie mon environnement Windows : `rustup`, `cargo`, la chaîne d'outils MSVC et `git`. S'il manque quelque chose, dis-moi exactement quoi installer et arrête-toi là.
3. Si un point de la spécification te paraît ambigu, contradictoire ou risqué, pose-moi tes questions **avant** d'écrire du code (cinq au maximum, les plus importantes).
4. Propose un **plan détaillé pour le jalon M0** : structure du workspace, dépendances avec leurs versions (utilise les dernières versions stables et fige-les), modules de `nascio-core`, liste des tests. Indique aussi brièvement comment tu envisages M1. Attends ma validation.
5. Une fois le plan validé, implémente M0. Termine par `cargo fmt`, `clippy` et les tests, puis fais un résumé de ce qui a été fait, de ce qui reste incertain, et des commandes pour vérifier par moi-même.
6. Fais un commit par étape logique (messages en anglais). Ne pousse vers GitHub que si je te le demande.

Ensuite, nous enchaînerons sur M1 (tectonique et visualiseur 2D), dans une nouvelle demande.
