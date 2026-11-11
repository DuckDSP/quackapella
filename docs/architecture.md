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
traité sont retardés de la même latence (fixée par le réglage *Voice*,
voir « Latence » plus bas), donc mix et bypass
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
- **non voisé** (s, f, ch, souffle) : jamais transposé (du bruit
  transposé bourdonne), mais le Formant s'y applique : chaque grain est lu
  plus vite ou plus lentement, à espacement inchangé. Le timbre est donc
  décalé dès le premier échantillon d'une phrase, sans attendre que le
  détecteur reconnaisse la note : seule la hauteur arrive ensuite. Un détecteur de bruit (part
  d'énergie dans les aigus, sur 2 ms) force le non voisé sur les
  consonnes et les éclats de plosive, même si le détecteur de pitch les
  croit voisés : sinon ces éclats, recouverts par des grains qui se
  chevauchent, sont doublés (surtout avec un Formant non nul) ;
- niveau gardé à peu près constant (gain `√(formant / ratio)` par grain) ;
- **attaques** : les premiers grains d'une note glissent en 12 ms de la
  hauteur chantée vers la hauteur visée (un petit scoop, au lieu d'un saut
  qui claque ; le Formant, lui, ne glisse pas), une coupure du détecteur de moins de 10 ms garde la
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

**Latence.** Un grain commence à sonner dès que sa marque est posée et
lit sa moitié droite pendant qu'il joue (l'entrée arrive aussi vite
qu'elle est jouée) : la voix n'a besoin que de sa moitié gauche (une
période) plus le temps de pose d'une marque (la recherche de crête
regarde 0,3 période après la crête prévue, qui peut arriver 0,3 période
en avance), soit 1,6 période. Auparavant le grain entier était calculé
d'avance (3,3 périodes : 30 ms à 110 Hz). La latence déclarée est celle
de la note la plus grave du réglage *Voice* (id 100) :

| Voice | Note la plus grave | Latence |
| --- | --- | --- |
| Soprano / Alto | F3 (175 Hz) | 9,2 ms |
| Tenor (défaut) | C3 (131 Hz) | 12,3 ms |
| Baritone | G2 (98 Hz) | 16,4 ms |
| Bass | D2 (73 Hz) | 21,9 ms |

Plus aiguë, la voix garde cette latence (sec et traité restent alignés) ;
plus grave, elle est retardée un peu plus (1,6 période), sans clic.
Changer de *Voice* change la latence déclarée (l'hôte est prévenu : AU v2,
CLAP, VST3) et vide l'audio un instant : c'est un réglage de mise en
place, pas de jeu. La taille du bloc hôte (32 ou 64 échantillons) n'y
change rien : tout est calculé échantillon par échantillon.

Mesure : `cargo run --release -p quack-dsp --example latency` (tableau
par voix et par note, blocs de 32 et 64), `-- PRISE.wav` (retard moyen et
pire sur une vraie prise), `-- --attack` (coût en qualité de l'attaque
courte).

**Low Latency** (id 101, désactivé par défaut) : les grains montent sur
une demi-période au lieu d'une période entière, puis restent pleins
jusqu'au grain suivant (la somme reste exacte sans transposition). La
latence tombe à 1,1 période : Soprano / Alto 6,4 ms, Tenor 8,5 ms,
Baritone 11,3 ms, Bass 15,1 ms. En contrepartie, les formants sont un peu
moins fidèles (environ 1 dB d'écart d'enveloppe en plus en moyenne, 7 dB
au pire sur une voyelle de synthèse), et le choix se fait à l'oreille.

À la reprise d'une note, les ~10 ms que met le détecteur à se décider
passent sans transposition, puis le glissé prend le relais.

Tests : transposition de -12 à +12 demi-tons à moins de 10 cents
(voyelle synthétique de 130 à 330 Hz), formants en place à ±1 octave
(centroïde spectral) mais déplacés quand on le demande, transparence
sans transposition (bruit à l'échantillon près, voyelle à -23 dB
d'erreur), voix grave à 70 Hz, pas d'allocation dans `process`.

## Autotune (étape 4)

`autotune::Retuner` corrige **grain par grain, sur la période propre du
grain** (distance entre deux impulsions de la forme d'onde), pas sur la
lecture du détecteur qui a ~25 ms de retard : la correction est en phase
avec la voix, le vibrato n'est jamais remis à contretemps. Le shifter
l'appelle via le trait `shift::PitchControl` (un `f32` reste un rapport
fixe).

- Notes permises : gamme (10 gammes) dans sa tonalité, moins les notes
  désactivées (12 interrupteurs) ; ou notes MIDI tenues (classes de
  hauteur, toute octave) ; ou **MIDI Exact** (la note jouée, quelle que
  soit la note chantée). Sans note tenue, la gamme reprend la main.
- Correction : `sortie = chanté − Correction × passe-bas(écart − Vibrato ×
  vibrato)`, constante de temps = Retune (+ Humanize sur les notes
  tenues). À 0 ms les grains sont posés à exactement une période cible :
  hard-tune net, justesse au cent près, sauts de note propres.
- Vibrato : un détecteur d'oscillation (alternances à 2,5-10 Hz) ouvre
  une porte ; un scoop ou une dérive ne sont pas pris pour du vibrato et
  sont corrigés.
- Choix de note avec hystérésis (0,2 demi-ton au-delà du milieu) et phase
  d'attaque de 20 ms où la note est rechoisie librement ; un saut de plus
  de 3 demi-tons ou un silence de 60 ms recommence une note.
- Deux caractères distincts :
  - **Natural** (correction qu'on n'entend pas) : Retune ≥ 30 ms, Vibrato
    ≥ 50 %, hystérésis large (0,35 demi-ton), correction qui s'installe
    sur les 80 premières ms de chaque note (scoops et notes courtes
    gardés), pas de Transition.
  - **Tune** (l'effet) : toute la plage, hystérésis 0,2, **Transition**
    (0-300 ms) : la cible glisse d'une note à l'autre en ce temps,
    indépendamment du Retune ; 0 = saut net.
- **MIDI Gate** : avec une source MIDI, la sortie est coupée tant
  qu'aucune note n'est tenue (fondus de 8 ms, retardés de la latence).
- Corrigé au passage dans le shifter : chaque grain était retardé d'un
  demi-échantillon en moyenne (sortie basse de 4 à 12 cents).

Rendu hors ligne : `cargo run --release -p quack-dsp --example render --
IN.wav OUT.wav mode=1 retune=0 key=9 scale=2` (clés des paramètres,
`midi=60,64@1.5` pour tenir des notes) ; `--demo OUT.wav` écrit un
chanteur de synthèse ; `--track IN.wav` affiche la hauteur toutes les
10 ms.

## Modes façon Manipulator (étape 5)

Tout est actif **seulement en mode Creative** : en Natural et en Tune les
réglages 600 à 609 sont ignorés (un test le vérifie à l'échantillon
près), le mode Natural reste naturel. **Aucun effet n'ajoute de
latence** : tout travaille dans le budget de 10 ms déjà gelé.

- **Couches d'octave** (*Sub* −2 oct, *Octave Down* −1 oct, *Octave Up*
  +1 oct, 0-100 %) : construites sur les grains de la voix principale,
  sur ses propres marques de synthèse (une sur deux à −1 octave, une sur
  quatre à −2, deux fois plus à +1), avec le même grain d'analyse. Leurs
  impulsions tombent donc sur celles de la voix : pas de fondamentale
  annulée là où leurs harmoniques se rencontrent (c'était le cas avec des
  voix indépendantes). Elles suivent la hauteur corrigée et transposée,
  se taisent sur les consonnes et le souffle, et la somme est compensée
  (1/√(1 + Σ niveau²)) pour ne pas saturer.
- **Formant Follow** (0-100 %) : part du décalage de hauteur que suivent
  les formants. 100 % = rééchantillonnage : chipmunk vers le haut, voix
  de monstre vers le bas ; le bouton Formant s'ajoute par-dessus.
- **Freeze** : garde le dernier grain voisé (ou attend le prochain) et le
  rejoue en continu, fondu de 25 ms à l'entrée et à la sortie. Une note
  MIDI tenue le **joue à sa hauteur** (Pitch par-dessus) ; sans note, il
  garde la hauteur qu'il avait. Formant et Formant Follow s'y appliquent
  en direct.
- **Robot** (0-100 %) : voix monocorde. Chaque phrase (après 60 ms sans
  voisement) est tenue sur sa première note corrigée, ou sur la note MIDI
  tenue. Entre 0 et 100 %, mélange géométrique avec la correction.
- **Glitch** : rien n'est laissé au hasard. *Glitch Mode* choisit
  l'effet (Repeat, Stutter, Reverse, Tape = demi-vitesse), *Glitch Rate*
  la longueur d'un pas (1/4 à 1/32, sur le tempo et les mesures de
  l'hôte), *Glitch* la densité : la part des pas de chaque mesure qui
  sont glitchés, toujours les mêmes (le dernier pas d'abord, comme un
  fill, puis répartis sur la mesure), donc une mesure sonne pareil à
  chaque fois et monter le bouton ne fait qu'ajouter des pas. L'éditeur
  montre les pas allumés. *Glitch Hold* glitche tous les pas tant qu'il
  est enfoncé (bouton de jeu live, automatisable). Des pas glitchés à la
  suite gardent la tranche attrapée (beat repeat). Lecture dans le passé,
  fondus de 3 ms : aucune latence, pas de clic.

Le panneau **Manipulate** remplace le graphe de hauteur en mode Creative.
Rendu hors ligne : `key=valeur@t` change un réglage au temps `t`
(`freeze=1@1.0 freeze=0@4.6`). Rendus d'écoute dans `ecoute/creative/`.

Plus tard : morphing entre réglages (étape 8, avec presets et macros),
analyse partagée par l'harmoniseur (étape 6, sur la même mécanique que
les couches).

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
   (cœur du mode Natural), formant séparé ; latence ensuite ramenée à
   1,6 période de la note la plus grave (réglage *Voice*).
4. Autotune : gammes, notes MIDI, vitesse de retune, humanisation.
5. **Modes façon Manipulator** : couches d'octave, formants qui suivent,
   freeze jouable au MIDI, robot, glitch au tempo.
6. Harmoniseur MIDI polyphonique.
7. Vocodeur et talkbox avec porteuse interne.
8. Interface complète, presets, macros, MIDI learn, zoom.
9. Optimisation CPU et tests en live.

## Vérifications

Volontairement légères (demande de Vincent) : tests ciblés sur le DSP et
sur les invariants hôte (latence réelle = déclarée, paramètres conformes,
MIDI, pas d'allocation), `auval` et `pluginval` en CI macOS. Les essais
dans Logic restent manuels.
