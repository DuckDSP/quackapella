# Architecture

## Couches

```text
quack-plugin   adaptateur Truce : paramètres hôte, MIDI, buffers, latence
    │                     (aucun DSP)
    ├── quack-ui     éditeur egui : ParamAccess + View, thème DuckDSP
    └── quack-dsp    moteur : Engine, params, latency, midi, mode,
                     pitch, shift, autotune, harmony, vocoder
```

- `quack-dsp` ne dépend d'aucun framework : le même code tourne dans le
  plugin, les tests et de futurs outils hors ligne (rendu de fichiers,
  comparaisons avec Manipulator).
- Règles temps réel : allocation seulement dans `prepare`, pas de verrou
  ni d'E/S sur le thread audio. Le test `process_does_not_allocate`
  (`--features quack-plugin/rt-paranoid`) le vérifie pour l'adaptateur.
- `quack_dsp::params` est la source de vérité des paramètres ; la
  déclaration Truce est vérifiée contre elle par test. Plages d'ids
  réservées : 100 détection, 200 pitch/formants, 300 autotune,
  400 harmoniseur, 500 vocodeur, 600 effets façon Manipulator.

## Chaîne (étape 1)

Entrée → gain d'entrée → chaîne vocale (identité pour l'instant) → mix
dry/wet → gain de sortie → fondu de bypass. Le chemin sec et le chemin
traité sont retardés de la même latence fixe (10 ms), donc mix et bypass
ne créent jamais de filtrage en peigne, et la compensation de l'hôte est
toujours juste. Les notes MIDI (note on/off, CC 120/123) sont appliquées à
leur position dans le bloc et tenues dans une pile de 16 notes, lue par
l'éditeur (clavier) et, plus tard, par l'autotune et l'harmoniseur.

## Modes

| Mode | Intention | Formants |
|---|---|---|
| **Natural** | Corriger ou transposer sans que ça s'entende, même sur de grands intervalles | toujours préservés |
| Tune | Autotune classique, du discret au hard-tune | préservés |
| Creative | Façon Manipulator : pitch et formants séparés, monstre, chipmunk, freeze, glitch | libres |
| Synth | Façon VocalSynth : harmonies MIDI, vocodeur, talkbox | libres |

Le mode Natural répond à « Manipulator est parfois trop artificiel ». Ce
qui le rend naturel, prévu aux étapes 2 à 4 :

1. resynthèse synchrone de la période (famille PSOLA) guidée par la
   détection de pitch, plutôt qu'un vocodeur de phase (pas de son
   « phasy ») ;
2. enveloppe spectrale (formants) gardée en place quand la hauteur
   bouge, pour éviter l'effet chipmunk ou monstre ;
3. parties non voisées (s, f, ch, souffle) laissées intactes ;
4. correction modulée par la confiance du détecteur, vitesse de retune
   lente et humanisée, vibrato conservé.

## Feuille de route

1. **Socle** (cette étape) : workspace, AU v2 / VST3 / CLAP, passthrough,
   MIDI, latence déclarée, interface DuckDSP, script de bundle, CI.
2. Détection de pitch robuste et basse latence (YIN / pYIN), décision
   voisé / non voisé.
3. Pitch shift naturel avec préservation des formants (cœur du mode
   Natural), mesure et gel du budget de latence.
4. Autotune : gammes, notes MIDI, vitesse de retune, humanisation.
5. Formants indépendants et modes façon Manipulator (freeze, glitch,
   octaves).
6. Harmoniseur MIDI polyphonique.
7. Vocodeur et talkbox avec porteuse interne.
8. Interface complète, presets, macros, MIDI learn, zoom.
9. Optimisation CPU et tests en live.

## Vérifications

Volontairement légères (demande de Vincent) : tests ciblés sur le DSP et
sur les invariants hôte (latence réelle = déclarée, paramètres conformes,
MIDI, pas d'allocation), `auval` et `pluginval` en CI macOS. Les essais
dans Logic restent manuels.
