# ADR 0001 : socle, framework et formats

Statut : proposé le 2026-11-11 (étape 1), à valider par Vincent dans Logic.

## Contexte

Exigences : AU v2 prioritaire (Logic, macOS), puis VST3 et CLAP ; entrée
MIDI ; latence faible et constante pour le live ; interface dans la
direction graphique DuckDSP (Sonder, Knock). La proposition de départ était
nih-plug + clap-wrapper pour obtenir l'AU.

## Décision

**Truce 6.3.0, comme Sonder**, et non nih-plug + clap-wrapper :

- l'AU v2 de Truce est natif (shim C), déjà passé par `auval -strict` 5/5
  et par la checklist Logic de Sonder (ADR 0001 et 0003 de Sonder) ;
  clap-wrapper ajouterait CMake, l'AudioUnitSDK et un second hôte CLAP
  entre Logic et le plugin, pour un résultat non éprouvé chez DuckDSP ;
- nih-plug amont est en maintenance et sa reprise (nice-plug) n'a ni AU ni
  tests macOS poussés (évaluation de Sonder, 26/10) ;
- les patchs déjà faits pour Logic (redimensionnement AUv2, automatisation
  à l'échantillon près, flou après redimensionnement) sont repris tels
  quels dans `vendor/`, et l'interface réutilise le thème de Sonder (même
  famille visuelle sans tout redessiner).

Repli si Truce bloque : nice-plug (CLAP) + clap-wrapper (AU v2) avec le
même `quack-dsp` et le même `quack-ui` ; seul `quack-plugin` change.

Formats : `cargo truce build --au2 --vst3 --clap`. Codes AU : fabricant
`DKDS` (commun à DuckDSP), sous-type `Qkpl`, type `aumf` (effet avec
entrée MIDI, choisi par Truce dès que `midi_input = true`).

## Latence

Une latence fixe de **10 ms** est déclarée dès maintenant
(`quack_dsp::latency::LATENCY_MS`), quel que soit le mode. Les hôtes
gèrent mal une latence qui change ; le passthrough est donc retardé de
cette valeur par des lignes à retard (tests : la sortie est exactement en
retard de la latence déclarée, à 44,1, 48 et 96 kHz). Le budget sera
confirmé par les mesures de l'étape 3 (pitch shift naturel), puis figé
avant la première version.

## Conséquences

- Truce reste épinglé à `=6.3.0` ; toute montée de version refait la copie
  des crates vendorés, comme pour Sonder.
- Les codes AU et les identifiants de paramètres sont provisoires jusqu'à
  la première version, puis définitifs.
