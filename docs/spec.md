# Kiln — orchestrateur autonome de specs

Status: ready-for-agent

## Problem Statement

Lors de la construction d'une application comportant de nombreuses specs, le développeur doit constamment lancer des agents pour implémenter les tickets, déclencher les reviews, demander les corrections et intégrer les changements. Même après avoir documenté le produit et approuvé son périmètre, cette supervision manuelle empêche une exécution continue. TFT-Simulator est le cas d'usage motivant cette demande.

Le besoin est de confier à un orchestrateur une collection de specs approuvées et de retrouver un résultat intégré, vérifié et traçable, avec reprise possible après interruption.

## Solution

Kiln est un nouveau projet Rust, indépendant du CLI Node.js Auto Implements. Son moteur pilote une workflow run depuis les specs approuvées jusqu'à une branche d'intégration validée et une PR. Un CLI permet de préparer, lancer, consulter et reprendre une exécution ; une interface web locale expose son état.

Kiln orchestre explicitement le découpage en tickets, la vérification du découpage, les dépendances, les implementation sessions, les review gates, les debug cycles, l'intégration et la validation globale. Les skills de Matt Pocock fournissent les instructions aux agents ; le moteur Rust possède l'état et décide des transitions. Codex est le premier moteur d'agent pris en charge. Une interface d'adaptation permet d'ajouter Claude Code ultérieurement.

La première version prouve ce fonctionnement sur plusieurs specs liées dans un repo existant. Le but final est de pouvoir conduire l'implémentation d'une application entière ; aucune exécution ne doit être déclarée terminée simplement parce que des agents ont cessé de travailler ou que des tests locaux sont verts.

## User Stories

1. En tant que développeur, je veux fournir plusieurs specs approuvées, afin de lancer une exécution couvrant un ensemble cohérent de fonctionnalités.
2. En tant que développeur, je veux conserver mes specs en Markdown versionné, afin de connaître exactement les exigences utilisées.
3. En tant que développeur, je veux importer et synchroniser des issues GitHub, afin de travailler avec mon suivi existant.
4. En tant que développeur, je veux que les specs du repo fassent autorité en cas de divergence, afin que la synchronisation ne modifie pas silencieusement les exigences.
5. En tant que développeur, je veux obtenir automatiquement des tickets vérifiables et leurs dépendances, afin de ne pas préparer chaque instruction manuellement.
6. En tant que développeur, je veux qu'un agent distinct vérifie la couverture et les dépendances du découpage, afin de détecter les omissions avant l'implémentation.
7. En tant que développeur, je veux exécuter les tickets indépendants en parallèle, afin de réduire le temps d'attente.
8. En tant que développeur, je veux respecter les dépendances entre specs, afin de ne pas lancer des travaux dont les prérequis sont absents.
9. En tant que développeur, je veux isoler les changements de chaque implementation session dans Git, afin d'éviter les interférences entre agents.
10. En tant que développeur, je veux utiliser Codex comme premier moteur, afin de bénéficier de mon environnement habituel.
11. En tant que développeur, je veux pouvoir ajouter Claude Code via un adaptateur, afin de ne pas lier l'orchestrateur à un seul moteur.
12. En tant que développeur, je veux une review dans un contexte distinct de l'implémentation, afin que l'agent auteur ne juge pas seul son résultat.
13. En tant que développeur, je veux vérifier les standards du repo et la conformité aux specs, afin de détecter aussi bien les défauts techniques que les écarts fonctionnels.
14. En tant que développeur, je veux que Kiln lance les corrections et les nouvelles reviews, afin de ne pas relancer chaque debug cycle.
15. En tant que développeur, je veux intégrer les changements validés et revérifier leur combinaison, afin de détecter les incompatibilités entre tickets.
16. En tant que développeur, je veux vérifier les parcours impliquant plusieurs specs, afin de savoir si l'application fonctionne globalement.
17. En tant que développeur, je veux disposer d'une preuve de validation par spec, afin de comprendre ce qui justifie le résultat.
18. En tant que développeur, je veux distinguer vérifié, échoué et impossible à vérifier, afin de ne pas confondre une absence de preuve avec un succès.
19. En tant que développeur, je veux que les ambiguïtés soient arbitrées sans intervention, afin de permettre une exécution autonome.
20. En tant que développeur, je veux retrouver les arbitrages et leurs justifications, afin de comprendre les changements apportés aux specs.
21. En tant que développeur, je veux figer les specs pendant une exécution, afin qu'un changement de documentation ne modifie pas implicitement le travail en cours.
22. En tant que développeur, je veux replanifier explicitement après une modification de spec, afin de conserver le travail valable et invalider les résultats devenus obsolètes.
23. En tant que développeur, je veux reprendre après un crash ou une interruption, afin de ne pas perdre le travail déjà effectué.
24. En tant que développeur, je veux limiter la concurrence et les cycles de correction, afin de maîtriser les ressources et les boucles improductives.
25. En tant que développeur, je veux poursuivre les tickets indépendants lorsqu'un ticket est bloqué, afin qu'un échec local ne bloque pas tout le projet.
26. En tant que développeur, je veux configurer les limites de durée, d'usage et de coût disponibles, afin que l'exécution s'arrête proprement à leur épuisement.
27. En tant que développeur, je veux voir les agents actifs, dépendances, reviews, limites et blocages dans une interface web locale, afin de suivre l'exécution sans intervenir constamment.
28. En tant que développeur, je veux configurer les commandes, accès réseau et secrets autorisés avant le lancement, afin de permettre l'exécution dans un environnement isolé.
29. En tant que développeur, je veux recevoir une PR contenant le résultat intégré, afin de disposer d'un livrable consultable.
30. En tant que développeur, je veux choisir explicitement si Kiln peut fusionner ou déployer, afin que ces actions suivent la politique de mon projet.

## Implementation Decisions

- Kiln est un nouveau projet Rust, pas une réécriture incrémentale d'Auto Implements. L'ADR existant sur Claude Code et les tickets séquentiels décrit l'ancien produit ; il ne détermine pas l'architecture de Kiln. Les termes spec, ticket, workflow run, implementation session, review gate et debug cycle restent utiles.
- Le moteur est partagé entre le CLI et l'interface web locale. L'interface permet de suivre l'exécution et les éléments qui prouvent son résultat.
- Le moteur possède l'état durable, le graphe de dépendances et les transitions. Il ne délègue pas la gestion de toute l'exécution à une invocation opaque d'implement-spec.
- Les instructions de to-spec, to-tickets et implement-spec sont adaptées au contrat autonome. L'entrée de la première version reste une collection de specs déjà approuvées ; le grill et l'approbation initiale du produit ne sont pas automatisés.
- La génération des tickets utilise une validation par agent distinct à la place de la validation humaine du découpage. Chaque ticket doit être une tranche vérifiable ; les dépendances couvrent aussi les relations entre specs.
- Les specs Markdown versionnées du repo sont la référence. Les références GitHub sont conservées pour l'import et la synchronisation ; une divergence doit être visible et ne peut pas remplacer silencieusement une spec approuvée.
- Chaque workflow run conserve une version exacte de ses entrées. Les révisions décidées pendant la run sont tracées ; les modifications externes nécessitent une replanification explicite.
- L'ordonnanceur démarre uniquement les tickets dont les dépendances sont satisfaites. Les tickets indépendants peuvent avancer en parallèle, dans des worktrees et branches isolés. Une dépendance bloquée empêche ses descendants d'avancer, sans empêcher les travaux indépendants.
- Codex est le premier adaptateur. Le contrat d'adaptation doit permettre le lancement, le suivi du résultat et la gestion de l'arrêt et des erreurs, tout en laissant les décisions de progression au moteur Rust.
- Une review distincte examine Standards et Spec. Les corrections doivent être revérifiées avant intégration. Les vérifications après intégration et les parcours globaux déterminent la réussite de l'ensemble.
- Le vérificateur peut ajouter des tests dérivés des critères d'acceptation. Les preuves exécutables complètent les reviews ; un avis favorable d'agent ne remplace pas les vérifications requises.
- L'autonomie complète permet les arbitrages selon la hiérarchie objectifs produit, décisions d'architecture, specs. Chaque arbitrage et éventuelle révision de spec est conservé et soumis à vérification.
- Les limites sont configurables. Les valeurs initiales sont trois agents d'implémentation simultanés au maximum, trois cycles de correction par ticket, puis une tentative de replanification avant blocage si le problème persiste. La limite des implémenteurs ne constitue pas à elle seule une limite de tous les processus agents.
- La durée et l'usage sont limitables selon les données disponibles. Le coût monétaire exact n'est pas supposé disponible avec un abonnement ; toute estimation ou donnée absente doit être présentée comme telle. Aucun plafond monétaire par défaut n'a été fixé pendant la discussion.
- L'épuisement d'une limite conserve un état reprenable. La reprise réconcilie l'état enregistré avec les processus et les changements Git observables avant de poursuivre ; elle évite de reproduire des effets déjà accomplis.
- Chaque projet configure avant le lancement les commandes de build, tests et lancement, les critères d'acceptation, ainsi que les commandes, accès réseau et secrets autorisés dans son environnement isolé. Kiln vérifie la préparation avant de commencer.
- Les commits, push et PR sont automatisables sur une branche d'intégration dédiée. La fusion dans la branche principale et le déploiement sont des options explicites par projet, désactivées pour le scénario initial de livraison.
- Le stockage concret, le framework web, le mécanisme d'isolation et les protocoles détaillés des adaptateurs seront déterminés dans la conception technique ; aucun choix précis de bibliothèque n'est acquis.

## Testing Decisions

- Point de test principal confirmé par le développeur : le comportement externe d'une workflow run pilotée par le CLI dans un repo Git temporaire. Le test fournit plusieurs specs liées, un adaptateur d'agent déterministe et les commandes de vérification du projet ; il observe les résultats, l'état visible et les artefacts Git, plutôt que les fonctions internes.
- Ce point couvre le moteur, l'ordonnancement, la persistance, les review gates, les debug cycles et l'intégration via leur comportement observable. Il permet de simuler une implémentation incorrecte, une review défavorable, une correction, un échec d'intégration et une interruption suivie d'une reprise.
- Les scénarios doivent vérifier qu'un ticket ne démarre pas avant ses prérequis, que la concurrence est plafonnée, que les tickets indépendants continuent après un blocage et que les descendants bloqués ne sont pas lancés.
- La reprise doit être testée après interruption à plusieurs étapes ayant des effets externes, notamment entre un changement Git et son enregistrement, afin de détecter les duplications ou les pertes de progression.
- Les scénarios doivent vérifier les limites de correction, la tentative de replanification, l'épuisement des limites et la conservation d'un état reprenable.
- La validation doit détecter un cas où les tickets passent séparément mais où un parcours traversant plusieurs specs échoue. Ce cas ne doit pas produire une déclaration de réussite globale.
- Les résultats impossible à vérifier, les arbitrages et les invalidations causées par une modification de spec doivent être visibles et distincts d'une réussite.
- Les intégrations réelles Codex et GitHub nécessitent des vérifications ciblées de leurs contrats. Les tests déterministes du moteur ne prétendent pas démontrer la qualité d'un modèle réel.
- Un parcours de l'interface web vérifie qu'elle reflète l'état exposé par le moteur, y compris les reviews, preuves et blocages, sans recopier tous les scénarios du moteur.
- Les tests de l'ancien CLI apportent des exemples de comportement pour la reprise et les reviews, mais ne constituent pas des tests existants de Kiln. Le scénario pilote final utilise plusieurs specs liées d'un vrai repo ; son choix concret reste à faire.

## Out of Scope

- Modifier ou migrer le CLI Node.js existant dans cette spec.
- Automatiser le grill produit ou remplacer l'approbation humaine initiale des specs.
- Garantir qu'une application arbitrairement grande ou des specs insuffisantes peuvent être implémentées sans aucun blocage.
- Livrer l'adaptateur Claude Code dans la première version ; son ajout doit rester possible.
- Fournir un service hébergé multi-utilisateur ou une application desktop native.
- Faire de la fusion dans la branche principale ou du déploiement une étape obligatoire de la première version.
- Revendiquer un coût monétaire exact lorsque le moteur ne le fournit pas.
- Choisir ici des bibliothèques Rust, un schéma de stockage ou une technologie précise d'isolation.

## Further Notes

- Le contrat produit a été confirmé à l'issue du grill. Kiln est le nom de travail retenu ; sa disponibilité n'a pas été vérifiée.
- Cette spec décrit le produit et le périmètre de sa première version. Elle doit être découpée en tickets avant implémentation ; elle n'autorise pas à commencer immédiatement le développement.
- Le tracker local est utilisé pour ce document, conformément au fonctionnement existant du repo. Aucune issue distante n'est créée.
- Le développeur a confirmé l'approche de test ; la spec est publiée dans le tracker local avec le statut ready-for-agent.
