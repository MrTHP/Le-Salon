# Sécurité de Le Salon

## Versions prises en charge

Les correctifs de sécurité sont appliqués à la dernière version publiée
(actuellement **1.9.0**) et à la branche `main`.

## Ce que Le Salon garantit par défaut

- **TLS activé** et **vérification du certificat activée** pour les nouvelles
  configurations.
- **SASL PLAIN exige TLS + certificat vérifié.** Une connexion SASL sans TLS,
  ou avec un certificat non vérifié, est refusée avant toute connexion réseau.
  Le mot de passe n'est donc jamais envoyé en clair ni à travers un tunnel
  susceptible d'être intercepté par un MITM.
- **Aucun secret dans le fichier de configuration** lorsque le trousseau
  système est disponible (Secret Service, Keychain, Credential Manager). Le
  repli sans trousseau utilise `config.toml` en `0600` et un dossier en `0700`.
- **Secrets indexés par réseau** : un mot de passe mémorisé pour
  `(hôte, compte)` n'est pas réutilisé pour un autre réseau. Le champ est
  effacé dès que l'hôte ou le compte change.
- **Options CLI ponctuelles** : `--no-tls`, `--insecure` et
  `--sasl-password` ne modifient pas la configuration persistante et ne sont
  jamais enregistrés. Préférer `LE_SALON_SASL_PASSWORD` à `--sasl-password`,
  qui reste visible dans `ps`.
- **Fichiers privés** : `config.toml`, `scheduled.json`, `encryption.json`
  et `fish_keys.json` sont écrits en `0600`, dans un dossier `0700`.
- **Pas de télémétrie.** La seule requête HTTP sortante est le téléversement
  explicitement déclenché par l'utilisateur.
- **Aucun shell avec des données IRC** : les notifications utilisent des
  arguments séparés, les liens ouverts sont limités à `http(s)` et le client
  n'ouvre jamais de `file://`.
- **Contrôles de direction Unicode** retirés de l'affichage pour éviter
  l'usurpation visuelle d'un lien ou d'un message.

## Limites connues

Ces points sont des limites de conception d'IRC ou de services externes, pas
des vulnérabilités de Le Salon :

- **SASL PLAIN** transmet le mot de passe au serveur IRC (chiffré par TLS).
  Utilise un mot de passe dédié, jamais celui de ton compte principal.
- **Le serveur IRC voit les messages** : TLS protège le transport, pas le
  serveur. RelayCrypt (XChaCha20-Poly1305) protège le contenu entre deux
  clients Le Salon, et FiSH (Blowfish ECB) permet la compatibilité avec les
  autres clients, mais le serveur voit toujours les métadonnées et les
  messages non chiffrés.
- **FiSH (Blowfish ECB) n'est pas un chiffrement moderne** : pas
  d'authentification ni d'intégrité, motifs répétés visibles en ECB, clés
  échangées hors bande et sans forward secrecy. Il est proposé uniquement
  pour parler aux clients mIRC/HexChat/irssi/ZNC ; RelayCrypt reste
  prioritaire dès qu'une clé existe.
- **Historique local en clair** : les messages RelayCrypt/FiSH sont
  déchiffrés avant d'être écrits dans `~/.config/le-salon/history/`. Le
  dossier est protégé par les permissions de l'utilisateur, pas par une clé
  de chiffrement au repos.
- **DCC SEND n'est pas chiffré.** L'offre (IP, port) est relayée par le
  serveur IRC, qui peut se connecter avant le destinataire. N'envoie pas de
  fichier sensible par DCC.
- **Téléversement litterbox** : le fichier part chez `litterbox.catbox.moe`,
  reste 72 h et est accessible à toute personne ayant le lien. Une
  confirmation explicite est demandée, mais le service est public.
- **Toasts et notifications** : les toasts in-app affichent le contenu des
  messages pendant quelques secondes. Ils sont désactivables dans
  *Préférences* ; les notifications système sont également désactivables.
  Les textes sont nettoyés (codes mIRC, contrôles, caractères de direction)
  avant affichage.
- **Pas de bac à sable** : l'AppImage tourne avec les droits de l'utilisateur.
  Ne la lance jamais avec `sudo`/`root`. Pour isoler, utilise Flatpak ou
  Firejail.
- **Dépendances graphiques** : GPUI et ses dépendances représentent une
  surface d'attaque importante. `cargo audit` est exécuté en CI ; les
  dépendances non maintenues sont suivies, mais aucune garantie absolue ne
  peut être offerte.
- **Avis `grid` (GHSA-38c5-483c-4qqp)** : `taffy 0.9` embarque `grid 0.18`,
  mais n'appelle jamais `Grid::expand_rows` (aucun site d'appel dans le code
  de taffy). L'avis n'est donc pas déclenchable par les données IRC ; il sera
  supprimé lors de la montée de GPUI/taffy. Le reste des alertes
  `cargo audit` concerne des crates non maintenues, pas des failles
  exploitables connues.

## Signaler une vulnérabilité

Merci de **ne pas ouvrir d'issue publique** pour une vulnérabilité non
corrigée. Utilise l'onglet *Security → Report a vulnerability* du dépôt
GitHub :

<https://github.com/MrTHP/Le-Salon/security/advisories/new>

Indique la version, la plateforme, les étapes de reproduction et l'impact
estimé. Une réponse initiale est visée sous 7 jours.

## Vérifier une release

Les AppImage sont construites par la CI GitHub à partir du tag correspondant.
Compare le SHA-256 publié dans la note de release avec le fichier téléchargé :

```bash
sha256sum Le-Salon-1.9.0-x86_64.AppImage
```

Ne télécharge les binaires que depuis la page officielle des releases.
