## Project context

This project keeps a **context layer**: the knowledge that is not in the code —
the vision, the features and modules it is made of, and the decisions,
constraints and facts recorded about them, each with who decided it and when.
Four extra RPC ops read and write it, over the same `$FLETCH_RPC_DIR` mailbox
and with the same request/response shape as every other op (see the RPC
protocol above). Refer to entities by **slug**.

When anything has been recorded, this block ends with the project's
**overview**: the vision, the architectural and business constraints adopted
for it, a one-line legend per module (slug, summary, where it lives), and every
other entity by kind. It is the same for every agent on the project and is cut
to fit; a "Truncated" warning at its end means `context_get` has more.

### `context_get` — read before you act

Call it at the start of a task and before changing a feature, with `entities`
(slugs from the overview) and/or `paths` (repo-relative paths you are working in);
add a free-text `query`, or `include_history: true` to see what each decision
replaced. No args returns the map of the project. `stdout` is markdown: the
vision, the entities in play, the current decisions about them, and warnings for
anything provisional or contradicted. Prefer it over reading docs to learn what
the product is and why it is built this way.

### `context_record_decision` — write what was decided

```json
{"op":"context_record_decision","args":{"kind":"constraint","domain":"architectural",
 "statement":"Payments never touch the main database","rationale":"PCI scope stays in one service",
 "about":["payments"],"user_quote":"payments must never touch the main database"}}
```

- `kind` is `decision | constraint | fact` (default `decision`); `domain` is
  `business | architectural | implementation`; `stance` is `adopted` (default)
  or `rejected`; `about` names at least one entity by slug; `paths` anchors it
  to code. Statement up to 300 characters, rationale up to 1000.
- When the **user** states a constraint, a decision, or a fact about the
  business, pass `user_quote`: their words, verbatim. Fletch checks the quote
  against the user's own messages in this workspace; a match records *the
  quote itself* as the statement, confirmed as the user's (leave `statement`
  out, put your reading in `rationale`); a miss refuses the call. Never
  paraphrase into `user_quote` and never put your own words there.
- In a workspace with several checkouts, say which one a decision is about
  with `repo` (the checkout's folder name): it is settled by that repo's PR.
- When **you** decide something non-obvious or deviate from the plan, record
  that too. It lands provisional until this branch merges.
- **Related decisions are a two-step.** If current decisions already exist
  about the same entities in that domain, nothing is written: the reply
  carries `conflict: "related"` and those `heads`. Read them, then resubmit
  with `supersedes: {"id","reasoning"}` to replace one, `contradicts: ["<id>"]`
  to record a tension, or `coexists: true` when they all hold. A restatement
  of an existing head answers `already_recorded`.

### `context_record_entity` — a feature, module or topic the overview lacks

`{"slug":"billing","kind":"feature","name":"Billing","summary":"…","paths":["src/billing"],
"relates":[{"rel":"part_of","to":"payments"}]}` — `kind` is
`vision | goal | capability | feature | module | topic` (default `topic`);
summary up to 600 characters; `aliases` lets `context_get` find it under other
names. Pass `id` (the entity's current slug will do) to revise an existing
entity; a revision replaces every field, so send its `paths` and `aliases`
again. Record the entity before you record a decision about it.

### `context_link` — relate two entities

`{"from":"billing","to":"payments","rel":"part_of"}` — `rel` is
`part_of | depends_on | serves`; `remove: true` unlinks.
