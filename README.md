# gemini-api

**Ton quota Gemini CLI, exposé en API OpenAI-compatible, sur ta machine.**

Un seul binaire Rust : tu le lances (ou double-cliques le `.app` sur macOS), ton
navigateur s'ouvre pour te connecter à Google, et ensuite tu as un serveur
`http://localhost:8080/v1` compatible OpenAI qui **mimique Gemini CLI** en
face de l'API interne Google Code Assist (`cloudcode-pa.googleapis.com/v1internal`).

- **Mono-utilisateur** : ton compte Google, ton quota (tier gratuit : 60 req/min,
  1000 req/jour par compte). Les 429 de Google sont transmis tels quels.
- **Zéro dépendance** : pas de base de données, pas de Docker. Les tokens OAuth
  sont stockés dans un fichier local (`~/Library/Application Support/gemini-api/`
  sur macOS, `~/.config/gemini-api/` sur Linux), permissions 0600.
- **Léger** : ~15 Mo de RAM, démarrage instantané.

> ⚠️ `v1internal` est une API **non documentée** utilisée par Gemini CLI en
> interne ; Google peut la changer à tout moment. L'OAuth utilise le client ID
> public de Gemini CLI (flow PKCE loopback). Le tier gratuit Code Assist est
> prévu pour un usage **personnel** (ToS Gemini CLI).

## Démarrage

```bash
# depuis les sources
cargo run --release
```

**Premier lancement** : le navigateur s'ouvre sur Google → tu autorises →
Google redirige vers l'app (rien à copier-coller) → c'est prêt.
**Lancements suivants** : le serveur démarre direct (tokens rafraîchis automatiquement).

La page d'accueil `http://localhost:8080/` affiche : compte connecté, tier,
quota restant, exemple curl, et un bouton **Quitter**.

## macOS (.app)

Le dépôt ne contient pas le binaire : le workflow GitHub Actions
(`.github/workflows/build.yml`) build le `.app` sur runner macOS et l'attache
aux releases. En local sur un Mac :

```bash
cargo build --release && ./scripts/make-app.sh   # → "Gemini API.app"
```

L'app tourne sans icône Dock (`LSUIElement`) : on l'arrête depuis la page
d'accueil (bouton Quitter) ou `pkill gemini-api`.

**Première ouverture d'un .app téléchargé** : macOS Gatekeeper le bloque
(non signé par un developer Apple). Clic droit → *Ouvrir*, ou :

```bash
xattr -d com.apple.quarantine "Gemini API.app"
```

## Utilisation (compatible OpenAI)

```bash
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gemini-2.5-flash",
    "messages": [{"role": "user", "content": "Bonjour !"}],
    "stream": true
  }'
```

Compatible avec n'importe quel client/SDK OpenAI (base URL `http://localhost:8080/v1`,
n'importe quelle valeur en clé API par défaut).

- **Streaming SSE** OpenAI (`chat.completion.chunk`, `data: [DONE]`), keep-alive 15 s
- **Function calling** : `tools`, `tool_choice` (`auto`/`none`/`required`/nommé), `tool_calls`
- Multimodal : `image_url` avec data-URI base64 → `inlineData` (les URL distantes ne sont pas téléchargées)
- `response_format` : `json_object` et `json_schema`
- Alias de modèles : `gpt-4o`, `gpt-4o-mini`, `gpt-4.1`, `gpt-5`, `o3`, `o4-mini`… → gemini
- Les parts « thought » (raisonnement) sont exclues des réponses

| Endpoint | Description |
|---|---|
| `POST /v1/chat/completions` | Complétion (streaming ou non) |
| `GET /v1/models` | Modèles disponibles |
| `GET /v1/account/status` | Email, tier, projet, quota Google |
| `GET /` | Page d'accueil (statut, quota, Quitter) |
| `GET /login` | Relancer le login Google (après révocation) |

## Comportement opérationnel

- **Erreurs Google avant stream** → vrai statut HTTP (401/403/429 + `Retry-After`/502/504).
  **Erreurs en milieu de stream** → event SSE `{"error":...}` puis `[DONE]`
  (les headers 200 sont déjà partis), comme OpenAI.
- **Timeouts** : premier token 60 s, idle inter-chunks 120 s, deadline stream 10 min,
  réponse non-stream 120 s. Retry (×2, backoff+jitter) sur 5xx/réseau avant le premier chunk.
- **Concurrence** : max 10 requêtes en vol (configurable), sinon 429 + retry-after.
- **Refresh token** single-flight, rafraîchi 60 s avant expiration ; `invalid_grant`
  → 401 « re-login » et la page d'accueil propose le bouton adéquat.
- **Onboarding Code Assist** (`loadCodeAssist`/`onboardUser` + polling LRO) rejoué
  automatiquement si incomplet ; gère `NEEDS_VALIDATION` (URL de validation Google).

## Variables d'environnement

| Variable | Rôle | Défaut |
|---|---|---|
| `PORT` | Port du serveur (fallback +1…+9 si occupé) | `8080` |
| `GEMINI_API_KEY` | Exiger `Authorization: Bearer <clé>` sur `/v1/*` | vide |
| `GEMINI_API_DATA_DIR` | Dossier credentials + logs | config OS |
| `GEMINI_API_MAX_INFLIGHT` | Max requêtes simultanées | `10` |
| `GOOGLE_CLOUD_PROJECT` | Force le tier STANDARD avec ton projet GCP | vide (tier gratuit) |
| `GEMINI_CLI_VERSION` | Version User-Agent mimée | `0.16.0` |
| `CODE_ASSIST_ENDPOINT` | Endpoint Code Assist (tests) | URL Google |
| `GEMINI_AUTH_URL` / `GEMINI_TOKEN_URL` / `GEMINI_USERINFO_URL` | Endpoints OAuth (tests) | URL Google |
| `GEMINI_API_OPEN=1` | Ouvrir la page d'accueil au démarrage | auto sans terminal |

## Développement

```bash
cargo test          # 21 tests unitaires (converter, relay SSE, erreurs)
cargo run           # serveur en debug
```

Architecture (portage fidèle du projet Kotlin d'origine) :

```
src/
  main.rs            — démarrage, bind 127.0.0.1, restauration de session
  config.rs          — config + valeurs Gemini CLI (client ID, UA, alias)
  state.rs           — état partagé + credentials.json (0600)
  oauth.rs           — login loopback PKCE, échange/refresh de tokens
  codeassist/        — client v1internal : generateContent (stream SSE),
                       loadCodeAssist/onboardUser (LRO), retrieveUserQuota
  openai/            — DTOs OpenAI, convertisseur, StreamRelay SSE, service
  web.rs             — routes, page d'accueil, middleware clé API optionnel
```

## Limites connues

- `n > 1` non supporté (400)
- `image_url` http(s) distantes non supportées (data-URI uniquement)
- Un seul compte Google par installation (c'est le but)
