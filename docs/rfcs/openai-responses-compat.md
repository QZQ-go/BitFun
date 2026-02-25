# RFC: OpenAI Responses Compatibility for BitFun

## 1. Context

BitFun currently works primarily with OpenAI-Compatible `chat/completions` semantics. This causes compatibility gaps for providers and gateways that prioritize or only support OpenAI `responses`.

## 2. Goal

Introduce OpenAI `responses` compatibility without breaking existing users on `chat/completions`.

### In Scope

1. Add a new provider format: `openai_responses`.
2. Support `/responses` request body construction and stream parsing in backend.
3. Expose the new format in frontend model configuration.
4. Keep current `openai` behavior fully backward compatible.

### Out of Scope

- Full AI architecture refactor.
- Removal of legacy `openai` format.
- Breaking config migration.

## 3. Design Decisions

### D1. Compatibility-first format strategy

- `openai` = OpenAI-Compatible `chat/completions`
- `openai_responses` = OpenAI `responses`

### D2. URL normalization strategy

Support both:
- Full endpoint input (`.../chat/completions`, `.../responses`)
- API root input (`.../v1`) with endpoint auto-append by format

Rules:
- If `base_url` already ends with `/chat/completions` or `/responses`, use as-is.
- Otherwise:
  - `openai` -> append `/chat/completions`
  - `openai_responses` -> append `/responses`

### D3. Implementation strategy

Add dedicated `responses` request builder + stream handler. Do not patch old chat parser to handle both protocols.

## 4. Execution Plan

### Phase 1: Config contract extension

- Extend format enum/typing with `openai_responses`.
- Make frontend config UI support selecting and saving it.

### Phase 2: Endpoint routing and dispatch

- Add URL normalization in backend client.
- Split send logic by format (`openai` vs `openai_responses`).

### Phase 3: `/responses` request body builder

- Add message/input converter for Responses schema.
- Add `build_openai_responses_request_body`.
- Preserve `custom_request_body` override behavior.

### Phase 4: `/responses` streaming handler

- Add new stream handler/types module for Responses events.
- Minimum supported events:
  - `response.output_text.delta`
  - `response.function_call_arguments.delta`
  - `response.function_call_arguments.done`
  - `response.completed`
  - `error`
- Compatibility requirement for tool-call argument events:
  - Accept both `call_id` and `item_id` forms from OpenAI-compatible providers.
  - When only `item_id` is present, resolve `item_id -> call_id` from prior
    `response.output_item.*` events; if mapping is unavailable, fall back to
    using `item_id` as the tool-call identifier to avoid hard failure.
- Map to unified internal response shape so upper layers remain format-agnostic.

### Phase 5: Validation

- Rust unit tests:
  - URL normalization
  - Responses request body
  - Responses SSE event parsing
- Frontend checks:
  - type-check
  - web build

### Phase 6: Documentation

- Clarify difference between `openai` and `openai_responses`.
- Add migration notes and common misconfigurations.

## 5. Definition of Done

- Existing `openai` (`chat/completions`) remains unaffected.
- New `openai_responses` works with stable streaming.
- `base_url` root path auto-normalization works.
- Core and stream handler tests pass.
- Web type-check and build pass.
- Config + migration docs are updated.
- Responses parser tolerates provider schema variants for
  `function_call_arguments.*` (missing `call_id`, item-id-based deltas).

## 6. Risks and Mitigation

### R1: Parser regression for legacy format

- Mitigation: isolate new Responses parser/module.
- Rollback: revert only `openai_responses` branch files.

### R2: URL normalization mistakes

- Mitigation: add explicit tests for root/full endpoint/trailing slash cases.
- Rollback: temporarily require full endpoint.

### R3: Function-call argument delta assembly issues

- Mitigation: incremental buffering + JSON integrity checks.
- Rollback: degrade tool-call streaming to full-payload parsing temporarily.

### R4: OpenAI-compatible schema drift (`item_id` vs `call_id`)

- Mitigation: make parser identifier resolution tolerant (`call_id` first,
  then `item_id` mapping, then `item_id` fallback), and add regression tests
  for item-id-only payloads.
- Rollback: keep stream alive and ignore malformed deltas instead of aborting
  the full response.

## 7. Suggested Branch and PR Split

- Branch: `feat/openai-responses-compat`
- PR split suggestion:
  1. Config and type contracts
  2. Backend request builder and URL normalization
  3. Streaming parser and tool-call handling
  4. UI and docs
