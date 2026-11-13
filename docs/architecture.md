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
  400 harmoniseur (400 à 414), 500 vocodeur, 600 effets façon Manipulator.

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

## Harmoniseur (étape 6)

Actif **seulement en mode Synth** (comme les effets Creative en mode
Creative) : Natural, Tune et Creative ne l'entendent jamais. **Aucune
latence ajoutée** : les voix lisent la même entrée que la voix
principale, dans le budget déjà gelé.

- **Mêmes grains que la voix** : chaque voix d'harmonie (jusqu'à 8) a ses
  propres marques de synthèse, espacées de sa propre période, et prend le
  grain d'analyse de la voix le plus proche dans le temps. Le grain *est*
  l'enveloppe spectrale du chanteur : une harmonie sonne comme le même
  chanteur sur une autre note, formants en place, pas comme une copie
  rééchantillonnée. Les voix se taisent sur les consonnes et le souffle
  (pas de consonnes doublées), démarrent sur une marque de la voix
  principale et suivent sa correction.
- **Sources** (*Harmony*, id 400) :
  - **MIDI** : chaque note tenue est une voix, à sa hauteur (polyphonique,
    8 voix, vélocité → niveau, fondus de 15 ms / 80 ms) ;
  - **Scale** : jusqu'à 4 voix à intervalle fixe (*Harmony 1* à *4*, ids
    401 à 404, de −10e à +10e), comptés en notes de la gamme de l'autotune
    depuis la note de la voix : tierces majeures ou mineures là où il faut.
    En Chromatic, les intervalles comptent dans la gamme majeure de la
    tonalité ;
  - **Chord** : les mêmes intervalles, chacun ramené à la note la plus
    proche de l'accord tenu au MIDI (toutes octaves), du même côté de la
    voix, jamais sur sa note ni sur une autre voix. Sans accord : comme
    Scale.
- **Expression** (id 410) : part des inflexions de la voix copiée par les
  voix. En Scale / Chord, l'écart de la voix à sa note de référence
  (hystérésis 0,3 demi-ton) : une voix chantée un peu basse garde des
  harmonies parallèles. En MIDI, seulement ses mouvements rapides
  (vibrato, scoops ; au-delà de 150 ms, la note jouée fait foi).
  Limitée à ±1 demi-ton. À 0 : voix pile sur les notes (son plus
  synthétique).
- **Plusieurs chanteurs plutôt que des clones** : *Humanize* (id 408)
  retarde chaque voix de 4 à 18 ms qui errent lentement et la désaccorde
  de ±12 cents ; *Character* (id 409) donne à chaque voix son propre
  décalage de formants (jusqu'à ±2,4 demi-tons : d'autres gorges) ;
  *Spread* (id 407) les place dans le champ stéréo (puissance constante) ;
  *Glide* (id 411, 0-500 ms) les fait glisser d'une note à l'autre.
- **Doubleur** (*Double*, id 412) : deux prises de la voix elle-même, à
  gauche et à droite (au moins 60 % d'écart), en retard de 9 à 23 ms qui
  errent, désaccordées de quelques cents en sens opposés, avec leurs
  propres formants : un double tracking. Indépendant de la source
  d'harmonie (marche aussi avec *Harmony* sur Off).
- **Jeu au clavier** : *Close Voicing* (id 413, mode MIDI) ramène chaque
  note jouée à l'octave la plus proche de la voix, et l'y garde tant
  qu'elle reste à moins de 9 demi-tons (pas de saut d'octave quand la
  voix bouge un peu) : on joue les accords n'importe où sur le clavier.
  *Latch* (id 414) garde l'accord après relâchement, jusqu'au suivant.
  La pédale de sustain (CC 64) garde aussi les notes, dans tous les modes.
- **Niveaux** : *Lead* (id 405, la voix principale) et *Harmony Level*
  (id 406). Plusieurs voix s'additionnent sans s'empiler linéairement
  (quatre voix ≈ deux fois la puissance d'une).

Le panneau **Harmony** remplace le graphe de hauteur en mode Synth et
montre les notes que chantent les voix. Rendu hors ligne : `mode=3
harmony=2 harmony_1=11` (indices d'intervalle : 9 = Off, 11 = tierce
au-dessus, 4 = sixte en dessous) ; en mode Synth le rendu sort en stéréo.
Rendus d'écoute dans `ecoute/harmoniseur/`.

## Presets, macros, MIDI learn (étape 8)

- **Presets d'usine** (`quack_dsp::presets::FACTORY`, compilés dans le
  plugin) : 32 presets en cinq catégories (Natural, Tune, Creative,
  Harmony, Vocoder), chacun avec ses quatre macros. Un preset règle tout
  sauf ce qui dépend du chanteur et de la session : Bypass, *Voice*,
  Low Latency, gain d'entrée et tonalité (*Key*).
- **Presets utilisateur** : fichiers texte `.qkpreset` (`clé = valeur`,
  paramètres par leur clé stable du registre), dans
  `~/Library/Audio/Presets/DuckDSP/Quackapella/` sous macOS. Lus à
  l'ouverture du navigateur, écrits par *Save*. Une marque `*` signale
  un preset modifié.
- **Macros** : paramètres hôte 700 à 703 (automatisables). Chaque macro
  déplace jusqu'à 8 paramètres, chacun de -100 à +100 % de sa plage à
  fond, par-dessus la valeur réglée (bornée). L'adaptateur applique les
  macros aux valeurs avant le moteur (`QuackParams::engine_values`) :
  l'hôte garde la valeur de base, le bouton montre un arc fin jusqu'à la
  valeur modulée. Interdit aux macros : Mode, Bypass, Voice et Low
  Latency (ils changeraient la latence ou toute la chaîne).
- **MIDI learn** : clic droit sur n'importe quel contrôle, *MIDI learn*,
  puis bouger un contrôleur. Un CC par paramètre (macros comprises), le
  CC écrit directement la valeur du paramètre sur le thread audio
  (atomiques, sans allocation). CC 64 et 120 à 127 restent réservés.
- **Program Change** (désactivé par défaut) : le programme n rappelle le
  preset d'usine n, sur le thread audio.
- Macros, CC appris, preset chargé et zoom sont sauvés avec la session
  (`#[persist] control`, texte indexé par clé de paramètre).
- **Zoom** 75 à 200 % (rangée Macros) ; l'éditeur redimensionne la
  fenêtre et adopte la taille si l'hôte la change lui-même.

Aucune latence ajoutée : tout se passe au niveau des paramètres.

## Snapshots, morphing, sortie MIDI

Rangée *Snapshots* et panneau *MIDI Out*, au-dessus des macros (éditeur
1040 × 904). Paramètres hôte 1200 à 1212, hors presets.

- **Snapshots A et B** (`quack_dsp::snapshot`) : *Save to A / B* fige
  tous les réglages du son (ceux d'un preset, macros exclues). Sauvés
  avec la session, pas dans les presets.
- **Morph** (1200, 0 % = A, 100 % = B) avec *Morph On* (1201) :
  l'adaptateur écrit les paramètres eux-mêmes (l'éditeur, la session et
  le moteur suivent, le lissage du moteur fait le fondu). Les réglages
  continus sont interpolés, les interrupteurs et choix basculent à 50 %.
  Seuls les réglages différents entre A et B sont écrits, et seulement
  quand le morph bouge : un réglage commun reste libre, une retouche
  tient jusqu'au prochain mouvement. Pilotable à la main, par
  automation, par une macro, par MIDI learn.
- **Touches A/B** (*Snapshot Keys*, 1203) : deux notes MIDI (C0/C#0 par
  exemple) font glisser le morph vers A ou B en *Morph Time* (1202). Ces
  notes ne vont pas plus loin (ni harmoniseur ni autotune). Morph Off :
  les touches et les boutons A/B rappellent le snapshot d'un coup.
- **Sortie MIDI** (`quack_dsp::midiout`, 1210 à 1212) : note la plus
  proche de la voix, avec hystérésis (0,65 demi-ton tenu 15 ms) pour
  qu'un vibrato ne redéclenche pas ; fin de note après 40 ms sans voix ;
  vélocité selon le niveau d'entrée. Sources : *Sung*, *Sung + Bend*
  (pitch bend ±2 demi-tons qui suit vibrato et glissés), *Heard* (cible
  de l'autotune + transposition), *Heard + Harmony* (plus les notes de
  l'harmoniseur). Canal 1 à 16.
  - Envoyée par le port MIDI de l'hôte (`midi_output = true` dans
    `truce.toml`) pour les hôtes qui prennent le MIDI d'un effet
    (Reaper, Bitwig). Logic ne le fait pas : sous macOS le plugin publie
    aussi une source MIDI virtuelle « Quackapella » tant que MIDI Out
    est actif (`quack-plugin/src/vport.rs`, file sans verrou vidée par
    un fil toutes les ms ; le thread audio n'appelle jamais CoreMIDI).
  - Délai : la note part quand le détecteur accroche la voix, 9 ms
    (Soprano) à 19 ms (Bass) après l'attaque (test
    `midi_out_onset_per_voice_range`). Aucune latence audio ajoutée.

## Égaliseur de sortie, exciter, prises d'analyse

Première étape de l'assistant de mixage (ids 1300 et suivants).

- **Où** : après les couches et le chorus, avant le retour du delay et
  de la reverb et avant la dynamique de sortie. La couleur se règle sur
  le son transformé (formants, harmonies, vocodeur), pas sur la voix
  brute ; l'onglet Tone reste à l'entrée, pour nettoyer la source.
- **Égaliseur** (`fx::Eq`) : coupe-bas, plateau grave, trois cloches,
  plateau aigu, en filtres à variables d'état à phase minimale. Aucune
  latence. Une bande à plat ne coûte rien. `fx::EqCurve` donne la courbe
  jouée, la même que celle dessinée.
- **Exciter** (`fx::Exciter`) : harmoniques fabriquées à partir de la
  bande juste au-dessus de sa fréquence, ajoutées en parallèle, à niveau
  relatif constant. Aucune latence.
- **Prises d'analyse** (`tap`) : quatre anneaux de 4096 samples écrits
  par le fil audio (un atomique par sample, sans attente) : l'entrée,
  après les couches, la sortie des effets et l'entrée sidechain.
  L'éditeur y lit le spectre du graphe et l'assistant ses mesures. Rien
  sur le chemin audio.
- **Éditeur** : sur la page FX, le graphe de l'égaliseur remplace le
  panneau Tune (`quack-ui/src/eq.rs`). Les fréquences restent linéaires
  pour l'hôte, les boutons et le graphe sont logarithmiques.

## Assistant de mixage

`quack_dsp::assist` écoute quelques secondes de chant et propose des
réglages. Rien ne tourne sur le fil audio : l'éditeur (ou l'exemple
`render` avec `assist=Pop`) nourrit un `Listener` avec les deux prises
d'analyse et le pitch détecté. Aucune latence ajoutée.

- **Prise d'entrée** : ce qui nettoie la source. Tessiture (coupe-bas à
  0,8 fois la note la plus grave), bruit entre les phrases (gate 12 dB
  au-dessus), sifflantes (de-esseur, fréquence et dose), écart entre
  doux et fort (compresseur).
- **Prise après les couches** : la couleur. L'équilibre bas-médiums,
  présence et air contre les médiums est comparé à la cible du style,
  et l'écart règle l'EQ de sortie (cloche à 300 Hz, cloche à 3,5 kHz,
  plateau à 9 kHz, cloche étroite sur une résonance entre 1 et 5 kHz).
- **Styles** (`Style::rules`) : Natural, Pop, R&B, Trap, Rock, Electro,
  Ballad, Lo-Fi. Les cibles sont des conventions de mixage, réunies
  dans une seule table pour être réglées à l'oreille.
- **Espace et brillance** : chaque style a sa reverb (retour, taille,
  amortissement, coupe-bas, atténuation pendant le chant), son delay
  sur le tempo de l'hôte (valeur de note, réinjection, couleur,
  ping-pong, part envoyée dans la reverb, atténué pendant le chant) et
  sa dose d'exciter (aucune si la voix est déjà brillante, moitié si
  les sifflantes sont fortes). La queue et le pré-délai de la reverb
  suivent le tempo (`reverb_times` : valeurs du style à 120 BPM, queue
  bornée entre 0,6 et 1,6 fois, pré-délai sur une quadruple, triple ou
  double croche) ; la réinjection du delay baisse quand le tempo
  ralentit. Sans tempo de l'hôte : 120 BPM. Le tempo arrive à
  l'éditeur par `View::tempo`.
- **Trois doses** (`Amounts`) : Clarity (EQ, de-esseur, exciter),
  Control (compresseur), Space (envois de chaque couche vers la reverb
  et le delay : 35 % pour la voix lead à fond, 17 % à 70 %, harmonies plus loin
  dans la reverb, vocodeur presque sec ; les retours sont fixés par le
  style ; à zéro, l'espace n'est pas touché).
- `propose` est une fonction pure (mesures, style, dose) ; `describe`
  dit la même chose en cinq lignes. `PARAMS` liste tout ce qu'une
  proposition peut toucher (pour Undo).
- **Vérification en sortie** : une troisième prise (`Taps::output`,
  après l'EQ, les retours et la dynamique de sortie) donne ce que la
  chaîne ajoute après les couches, EQ mis à part (`Analysis::colour`,
  relatif aux médiums, borné à ±6 dB). Une deuxième écoute, proposition
  en place, corrige donc l'EQ de ce qu'apportent l'exciter et la
  reverb. L'éditeur enchaîne cette vérification tout seul après la
  première écoute (0,7 s pour laisser la proposition s'installer, puis
  4 s de chant), avec un message, et affiche ce qu'elle a changé.
  Quand le style ou une dose bouge ensuite, l'éditeur affiche
  « Changed: check again » et le bouton Check, allumé, refait la seule
  vérification (4 s de chant) sans refaire le diagnostic de la voix.
- **Tessiture** : `Analysis::bands` déplace les zones mesurées et les
  fréquences de l'EQ avec la médiane chantée, autour d'un ténor de
  référence (185 Hz : 300 Hz, 3,5 kHz, 9 kHz). Les bas-médiums suivent
  en racine carrée (240 à 420 Hz), présence et air en racine quatrième
  (3,1 à 4,3 kHz, plateau de 8 à 11 kHz).
- **Sortie** : le compresseur de sortie est dosé par style
  (`Style::glue`, avec Control), le plafond mis à -1 dB, et le
  de-esseur de sortie réglé à la vérification d'après les sifflantes
  mesurées en sortie (`Analysis::out_sibilance`).
- **Écoute du mix** : une quatrième prise (`Taps::side`) lit l'entrée
  sidechain. Quand elle est libre (ni Voice from Sidechain, ni porteuse
  externe du vocodeur), c'est le mix : s'il est chargé vers 3 kHz la
  cible de présence monte (jusqu'à +3 dB), s'il est plein dans les
  bas-médiums celle de la voix descend (jusqu'à -2 dB). Seule la forme
  du mix est lue, pas son niveau. Sidechain pris ou muet : l'assistant
  le dit et s'en passe.
- **Interface** (`quack-ui/src/assist.rs`) : sur la page FX, le panneau
  de gauche bascule entre le graphe de l'EQ et l'assistant. Listen,
  diagnostic, Clarity, Control et Space, puis Apply, Compare et Undo. La proposition
  s'entend dès la fin de l'écoute et suit le style et les trois doses.
  Compare bascule entre avant et après, Undo remet tout ; elle reste
  alors affichée en points blancs sur les boutons et Apply la rejoue.
  Un réglage fait à la main n'est pas écrasé.

## Quackapella Link (plugin compagnon)

Un second plugin (`crates/quack-link-plugin`, AU `aufx` `Qklk`, sans
MIDI) à poser sur une autre piste : il laisse passer l'audio sans y
toucher (latence nulle) et en envoie une copie à Quackapella. Logic ne
donne pas de menu Side Chain à un `aumf` en insert d'une piste audio,
et le side-chain ne porte qu'une source : Link règle les deux.

- **Rôles** (paramètre `Send As`) : `Mix`, le reste du morceau pour
  l'assistant ; `Carrier`, la porteuse du vocodeur (ou la voix avec
  Voice from Side-chain). Un seul émetteur par rôle : le second affiche
  « Another Link already sends… » et prend la place une demi-seconde
  après l'arrêt du premier. `Send Level` (-24 à +12 dB) ne règle que la
  copie envoyée.
- **Le fil** (`crates/quack-link`) : une région de mémoire partagée par
  rôle (`shm_open`, à défaut un fichier du dossier temporaire), parce
  que deux plugins sont deux bibliothèques et que certains hôtes les
  isolent dans deux processus. Un en-tête d'atomiques (émetteur,
  battement, compteur d'écriture, fréquence d'échantillonnage) et un
  anneau de 16 384 trames stéréo. Les deux bouts sont sans attente ;
  rien n'est ouvert ni alloué hors de `reset`.
- **Réception** (`quack-plugin`) : la porteuse reçue prend la place du
  side-chain de l'hôte (le moteur ne voit pas la différence) ; le mix
  reçu va dans la prise `Taps::link`, que l'assistant préfère au
  side-chain. Une fréquence d'échantillonnage différente n'est pas
  reçue.
- **Retard du son relié** : le récepteur lit en retrait du plus grand
  bloc de l'émetteur plus deux des siens (96 échantillons à 32, soit
  2 ms), pour qu'un hôte qui rend l'autre piste par gros blocs ne le
  laisse jamais à court. Ce retard ne touche que le son relié : la voix
  n'attend jamais le fil, et la latence déclarée ne change pas.
- **Éditeur** (`quack_ui::link`) : rôle, niveau, vu-mètre de ce qui
  part, et l'état du lien en bas (reçu, personne n'écoute, rôle pris).

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
6. **Harmoniseur** : voix sur les grains de la voix, MIDI polyphonique,
   intervalles dans la gamme, accords MIDI, humanisation, stéréo.
7. Vocodeur et talkbox avec porteuse interne.
8. **Interface complète, presets, macros, MIDI learn, zoom.**
9. **Optimisation CPU et tests en live** : voir « Charge CPU et
   robustesse » ci-dessous.

## Charge CPU et robustesse

Mesure : `cargo run --release -p quack-dsp --example bench` (options
`--sr 96000`, `--case synth`). Un chanteur synthétique passe dans chaque
configuration en blocs hôte de 32 et 64 échantillons ; le banc donne la
part d'un cœur utilisée en temps réel et le pire bloc (part de la durée
du bloc), chaque bloc gardant son meilleur temps sur cinq passes pour ne
pas compter les préemptions du système.

À 48 kHz, sur un cœur x86 de la machine de CI (le Mac est au moins aussi
rapide) :

| Configuration | Avant | Après | Pire bloc (32) |
|---|---|---|---|
| Natural | 2,0 % | 0,9 % | 2 % |
| Creative (octaves, robot, glitch) | 2,7 % | 1,6 % | 3 % |
| Synth, harmonies 4 voix + doubleur | 3,6 % | 2,5 % | 4 % |
| Synth, vocodeur 32 bandes, 8 notes supersaw | 4,2 % | 2,4 % | 4 % |
| Synth, talkbox | 4,3 % | 2,4 % | 6 % |
| Synth, tout ensemble | 5,7 % | 4,1 % | 6 % |

À 96 kHz, tout ensemble : 7,6 % (pire bloc 19 % en blocs de 32). La
taille du bloc ne change rien au coût : tout est calculé par
échantillon.

Ce qui a été optimisé (sans changer le son) :

- **YIN** : la fonction de différence (l'essentiel du détecteur) est
  sommée sur huit accumulateurs, ce qui la vectorise (SSE, NEON) ; les
  notes détectées sont identiques au millième de demi-ton sur la prise
  de Vincent.
- **Harmoniseur** : les voix éteintes ne coûtent plus rien, les gains de
  pan (sin, cos) et le formant de chaque voix ne sont recalculés que
  quand ils bougent (30 appels libm par échantillon avant, même
  harmoniseur coupé).
- **Porteuse du vocodeur** : rapports de désaccord des oscillateurs
  calculés une fois par réglage (40 `exp2` par échantillon avant en
  supersaw 8 notes), `fmod` remplacé par une soustraction exacte.
- **Dénormaux** : flush-to-zero pendant `process` (`denormal`), restauré
  ensuite pour l'hôte.

Le SIMD explicite n'a pas été ajouté ailleurs : le reste est fait de
filtres récursifs (non vectorisables par échantillon) et ne dépasse pas
quelques pour cent d'un cœur.

Robustesse (`crates/quack-dsp/tests/realtime.rs`) :

- aucune allocation dans `process`, les changements de paramètres (tous,
  sur toute leur plage, y compris Voice et Low Latency), le MIDI et le
  transport, dans les quatre modes, en blocs de 1, 32, 64 et 4096 ;
- NaN, infinis et niveaux absurdes en entrée (remplacés par du silence
  ou bornés à +30 dBFS), pitch bend, transport et paramètres NaN :
  sortie finie, le son revient dès que l'entrée est propre ;
- même sortie quelle que soit la taille des blocs, même plus grands que
  le maximum annoncé par l'hôte ;
- changement de sample rate (22,05 à 192 kHz) sur le même moteur ;
- silence après une prise : la sortie retombe à zéro, sans dénormaux ;
- chaque valeur de chaque paramètre à choix, sur le réglage Synth le
  plus lourd : sortie finie et bornée.

## Vérifications

Volontairement légères (demande de Vincent) : tests ciblés sur le DSP et
sur les invariants hôte (latence réelle = déclarée, paramètres conformes,
MIDI, pas d'allocation), `auval` et `pluginval` en CI macOS. Les essais
dans Logic restent manuels.
