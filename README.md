# Kiln

Kiln est un orchestrateur Rust qui conduit des specs approuvées jusqu'à une application intégrée et vérifiée, en pilotant les agents, les reviews, les corrections et les dépendances.

## État du projet

Conception produit approuvée. L'implémentation n'a pas commencé et le découpage en tickets reste à valider.

La [spec produit](docs/spec.md) décrit le périmètre de la première version : moteur Rust, CLI, interface web locale, Codex comme premier moteur d'agent, exécution parallèle et reprise après interruption.

## Parcours cible

Specs approuvées → tickets vérifiés → implémentation → review → corrections → intégration → validation globale → PR.

Le moteur Rust possède l'état de l'exécution. Les skills de Matt Pocock fournissent les instructions aux agents.
