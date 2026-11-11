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

## Détection de pitch (étape 2)

`quack_dsp::pitch::Yin`, lancé par le moteur sur l'entrée (après gain
d'entrée, somme mono), en lecture seule :

- filtre passe-bas 2 kHz (4ᵉ ordre) puis décimation vers environ 11 kHz,
  passe-haut 40 Hz ; plage 60 Hz à 1,5 kHz ;
- YIN toutes les 2,5 ms sur une fenêtre de deux périodes à 60 Hz
  (environ 33 ms), seuil de 0,15 sur la différence normalisée (évite
  les erreurs d'octave vers le bas), interpolation parabolique ;
- voisé / non voisé avec hystérésis (apériodicité < 0,25 pour entrer,
  > 0,40 pour sortir) et porte à -55 dBFS : consonnes, souffle et
  silence ne donnent aucune lecture ;
- à l'attaque, tant que la longue fenêtre contient encore le silence
  d'avant, une vérification courte compare seulement les deux dernières
  périodes (au moins 5 ms) : la note est lue environ 10 ms après son
  début au lieu de 30. Cette lecture rapide n'est acceptée que si elle
  est très périodique (apériodicité < 0,1) et confirmée par le pas
  suivant (à un demi-ton près) : une plosive (p, t, k) ne passe plus pour
  une note ;
- médiane sur 5 lectures, en demi-tons : un saut d'octave isolé est
  rejeté, un vrai changement de note passe 5 ms plus tard ;
- confiance = 1 - apériodicité, que le pitch shift (étape 3) et
  l'autotune (étape 4) utiliseront pour doser la correction.

La lecture suit la voix d'environ 25 ms (`latency_samples`). C'est un
retard de commande, pas de latence audio : l'étape 3 en tiendra compte
dans son budget de 10 ms (les marques de période du PSOLA seront
calculées sur le signal plein débit, guidées par cette lecture).
L'éditeur affiche la note, l'écart en cents, la fréquence et la
confiance.

## Pitch shift naturel (étape 3)

`quack_dsp::shift::Psola`, un TD-PSOLA temps réel guidé par le
détecteur, à la place de la ligne de retard du chemin traité :

- **marques d'analyse** : une par période, sur un pic fort du signal
  passe-bas (1 kHz) cherché à ±0,3 période autour de la position prédite
  (dernière marque + période lue), choisi par corrélation avec la période
  précédente pour rester sur la même impulsion d'une période à l'autre. Elles suivent l'onde elle-même, donc le retard
  de lecture du détecteur et le vibrato ne les brouillent pas. En non
  voisé, une marque toutes les 2,5 ms ;
- **grains** : fenêtre de Hann asymétrique qui va de la marque précédente
  à la suivante (deux périodes). Un grain contient une impulsion
  glottique et la réponse du conduit vocal : sa forme *est* l'enveloppe
  spectrale. Les grains voisins somment à exactement 1 sans transposition :
  le shifter est transparent quand il ne fait rien ;
- **synthèse** : grains replacés tous les `période / ratio`, chacun
  prenant le grain d'analyse le plus proche dans le temps (répété en
  montant, sauté en descendant). La hauteur bouge, la forme du grain non :
  les formants restent en place, sans effet chipmunk ou monstre, et sans
  le côté « phasy » d'un vocodeur de phase ;
- **formant séparé** : lire le grain plus vite ou plus lentement déplace
  l'enveloppe sans toucher la hauteur (±12 demi-tons) ;
- **non voisé** (s, f, ch, souffle) : toujours replacé tel quel, jamais
  transposé (du bruit transposé bourdonne). Un détecteur de bruit (part
  d'énergie dans les aigus, sur 2 ms) force le non voisé sur les
  consonnes et les éclats de plosive, même si le détecteur de pitch les
  croit voisés : sinon ces éclats, recouverts par des grains qui se
  chevauchent, sont doublés (surtout avec un Formant non nul) ;
- niveau gardé à peu près constant (gain `√(formant / ratio)` par grain) ;
- **attaques** : les premiers grains d'une note glissent en 12 ms de la
  hauteur chantée vers la hauteur visée (un petit scoop, au lieu d'un saut
  qui claque), une coupure du détecteur de moins de 10 ms garde la
  dernière période (pas de va-et-vient entre grains transposés et non
  transposés), et un grain plus large que le précédent est décalé plutôt
  que coupé ;
- **retard de contenu stable** : le retard entre l'entrée et le grain lu
  ne change plus à chaque passage consonne / voyelle (c'était la cause
  des « sauts » : un bout de son rejoué ou sauté). Il a un plancher qui
  dépend de la période chantée (lissée), ne redescend que lentement
  (3 % par grain), et quand le grain voulu n'est pas encore complet le
  shifter attend un peu (court blanc) plutôt que de rejouer un grain
  déjà entendu.

Réglages : *Pitch* (±24 demi-tons), *Fine* (±100 cents), *Formant*
(±12 demi-tons), lissés sur 20 ms dans le moteur. Ids hôte 200 à 202.

**Latence.** Le budget de 10 ms est gardé et gelé : la sortie est
l'entrée retardée d'exactement 10 ms. Un grain a besoin de ses deux
périodes complètes ; sous environ 120 Hz c'est plus que le budget, et le
shifter prend alors le dernier grain complet, ce qui retarde le contenu
des voix graves de quelques millisecondes de plus (sans clic, sans
changer la latence déclarée). À la reprise d'une note, les ~10 ms que met
le détecteur à se décider passent sans transposition, puis le glissé
prend le relais.

Tests : transposition de -12 à +12 demi-tons à moins de 10 cents
(voyelle synthétique de 130 à 330 Hz), formants en place à ±1 octave
(centroïde spectral) mais déplacés quand on le demande, transparence
sans transposition (bruit à l'échantillon près, voyelle à -23 dB
d'erreur), voix grave à 70 Hz, pas d'allocation dans `process`.

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

1. **Socle** : workspace, AU v2 / VST3 / CLAP, passthrough,
   MIDI, latence déclarée, interface DuckDSP, script de bundle, CI.
2. **Détection de pitch** : YIN, décision voisé / non
   voisé, confiance, lissage, affichage dans l'éditeur.
3. **Pitch shift naturel** (cette étape) avec préservation des formants
   (cœur du mode Natural), formant séparé, budget de latence gelé à 10 ms.
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
