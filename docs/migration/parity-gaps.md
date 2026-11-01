# Écarts de parité assumés

Divergences entre le test Ruby et le contrat observable sur le stack, retenues
exprès. Une ligne n'entre ici qu'avec une réponse réelle du stack Rails.

## Taille d'upload : 413 au bord, 422 dans le modèle

Les tests Ruby abaissent `Document::MAX_BYTE_SIZE` à 3 octets, sous la limite du
proxy, pour obtenir un 422 :

- `test/security/input_fuzz_test.rb:170` — un `Document` de 200 Mio est invalide
  (validation de modèle, pas de requête HTTP) ;
- `test/security/input_fuzz_test.rb:238` — upload synchrone rejeté en 422 ;
- `test/security/input_fuzz_test.rb:248` — upload asynchrone rejeté en 422, sans
  fichier de staging.

Sur le stack compose, `GED_MAX_UPLOAD_BYTES` (défaut 52 428 800) est à la fois
le plafond du modèle et `MAX_REQUEST_BODY` de Thruster (52_428_800 octets). Un corps plus grand
n'atteint pas Rails. Observé le 2026-11-01 avec
`GED_BACKEND=rails cargo test -p parity security` :

- statut **413**, corps **vide** ;
- aucune ligne `documents` ;
- aucune ligne `upload_requests` quand un `callback_url` est joint.

Le 422 du modèle n'est pas atteignable en boîte noire sans modifier Rails ou
baisser la limite en dessous du bord. La spec de migration retire Thruster et
fait renvoyer 413 par Axum avant toute écriture disque
(`docs/superpowers/specs/2026-11-01-rust-migration-design.md`, bord HTTP). Les
tests `security_fuzz_oversized_*` verrouillent ce contrat. L'absence du fichier
de staging reste unitaire (`rust/ged/src/storage/staging.rs`) : elle n'est pas
visible en HTTP.

Décision T2b (orchestrateur, après la revue) : le corps vide du 413 **reste
l'attente**. C'est le contrat observé sur le stack, à reproduire tel quel côté
Rust — pas un artefact de proxy à assouplir au portage. Un corps JSON
`{ "error": ... }` ou un texte du genre « length limit exceeded » serait une
divergence. L'attente reste : statut 413, corps vide (`body` de longueur 0),
aucune ligne `documents` ni `upload_requests`. Les tests
`security_fuzz_oversized_sync_upload_is_rejected_at_the_edge` et
`security_fuzz_oversized_async_upload_is_rejected_without_a_request_row` ne
changent pas.
