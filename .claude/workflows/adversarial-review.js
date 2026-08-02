export const meta = {
  name: 'adversarial-review',
  description: 'Isolated hostile critics over a diff, then a refutation panel each finding must survive before it is reported',
  whenToUse: 'Run by the adversarial-review skill, which collects the settings. args carry scope, tier (low|medium|high|max), triggers, contract and fileCount. If invoked with no args (a bare slash command), do not improvise: tell the user to run /adversarial-review.',
  phases: [
    { title: 'Critique', detail: 'one isolated hostile lens per critic' },
    { title: 'Refute', detail: 'a panel reads the deduplicated candidates in batches and votes on each' },
    { title: 'Adversarial', detail: 'max only: red-team the survivors on severity and reachability' },
  ],
}

// ---------------------------------------------------------------- settings

const TIERS = ['low', 'medium', 'high', 'max']

// The tier buys reasoning effort and the *width* of the panel. It never buys the panel
// away: VOTERS has a floor of 1, never 0.
const EFFORT = { low: 'medium', medium: 'high', high: 'xhigh', max: 'max' }
const VOTERS = { low: 1, medium: 1, high: 3, max: 3 }

// A voter reads a batch of candidates and votes on each, rather than one agent per
// candidate. Cost is 3*ceil(N/BATCH), not 3*N. Measured once, on a 7-file diff: six
// critics raised 53 candidates over 32 distinct sites, so per-candidate voting billed
// 159 agents - 21 of them re-judging sites another lens had already raised. The panel
// is the same panel; only the packing changed.
const BATCH = 10

// A single lens returning more than this is not being thorough, it is padding, and the
// long tail is what the panel then pays for. Kept candidates are the critic's own
// highest severities, and the drop is logged - never silently truncated.
const PER_LENS_CAP = 12

// A caller that stringifies its args gets a string here, not an object, and every field
// reads as undefined - which lands on the no-scope branch below and returns in
// milliseconds having reviewed nothing. Parse it rather than failing on a caller's
// serialisation choice.
let a = args || {}
if (typeof a === 'string') {
  try {
    a = JSON.parse(a)
    log('args arrived as a JSON string and were parsed; pass them as a JSON value to skip this.')
  } catch {
    a = {}
  }
}
if (!a || typeof a !== 'object') a = {}

if (!a.scope) {
  log('adversarial-review was started with no scope - nothing to review.')
  return { error: 'no-args', message: 'Run /adversarial-review, which collects the scope and tier.' }
}

const tier = TIERS.includes(a.tier) ? a.tier : 'medium'
if (a.tier && !TIERS.includes(a.tier)) {
  log(`unknown tier ${JSON.stringify(a.tier)} - using medium (tiers: ${TIERS.join(', ')})`)
}

const effort = EFFORT[tier]
const voters = VOTERS[tier]
const triggers = Array.isArray(a.triggers) ? a.triggers : []
const contract = a.contract || 'AGENTS.md, the matching docs/reference/ entry, and any task file under .claude/tasks/.'

// ---------------------------------------------------------------- lenses

// The host's finding card groups by category, not by lens - a reader wants to know what
// kind of defect this is, not which of our critics happened to raise it. Kebab-case, and
// stable: the chip text is what someone scans.
const CATEGORY = {
  saboteur: 'correctness',
  contract: 'contract-violation',
  newhire: 'maintainability',
  minimalist: 'simplification',
  trust: 'security',
  freya: 'ui-correctness',
  folded: 'correctness',
}

const LENSES = {
  saboteur: {
    title: 'Saboteur',
    mission: 'You are trying to break this in production.',
    hunt: 'The worst input: empty, huge or malformed parquet, a null Hive partition, a union column. What panics - unwrap/expect/indexing on anything data-derived. What happens if it runs twice, concurrently, or is cancelled halfway. Blocking work on the render thread. A reader outliving its Run without pin_snapshot. A cache entry left with no subscriber. Close or quit mid-operation.',
  },
  contract: {
    title: 'Contract lawyer',
    mission: 'This works, and it is still a violation.',
    hunt: `The diff against the written contract: ${contract} Name every rule the diff touches, with its section, and a verdict on each. Also the reverse: a change obliged to update a task file, a reference doc or themes/theme.schema.json in the same commit, that did not.

REQUIRED SEPARATE STEP - the negation pass. Models underweight negation, so a rule phrased "never X" is the one you will wave through, and AGENTS.md is written almost entirely in "never", "no" and "don't". Do not ask yourself whether a rule was violated. For each prohibition the diff comes near, turn it into a positive search and grep the diff for the forbidden thing itself: "never a shared registry value" becomes a search for a map keyed by tab id; "no command bus" becomes a search for a root-level handler registry. Let the search answer, rather than trying to notice an absence.`,
  },
  newhire: {
    title: 'New hire',
    mission: 'You joined today and must change this in six months, with nobody to ask.',
    hunt: 'Names that mean nothing (data, info, handle), names that mean two different things in one file, magic numbers, a path you cannot trace without three files open, a function doing more than its name admits, non-obvious logic with no comment, user-facing strings against the house tone rule (terse plain sentences, single-quoted identifiers, no em-dashes or ellipsis, no hedges), docs left stale by this change.',
  },
  minimalist: {
    title: 'Minimalist',
    mission: 'Delete it. What breaks?',
    hunt: 'A stub that passes today\'s case where the general mechanism was asked for. An adapter, echo field, parallel id or shim carrying an old framework shape across. A second producer or mirror of state that already has exactly one. Unreferenced pre-work. A capability another task owns, folded in locally instead of left inert. If deleting a thing breaks nothing, that is a finding.',
  },
  trust: {
    title: 'Trust auditor',
    mission: 'This boundary is the one that gets crossed.',
    hunt: 'A gate that does not run before dispatch, or does not fail closed. An agent writing window state it does not own. Typed DDL reaching the engine. A path escaping the project root. An export overwriting user data. Credentials in connection config, logs or error prose.',
  },
  freya: {
    title: 'Freya pedant',
    mission: 'This compiles, renders, and is silently wrong.',
    hunt: 'A second handler registration silently replacing the first (including the sugar family sharing names with primitives). A border painted without matching padding. Size::flex under a parent whose content is not Flex. A size on a node the parent does not lay out. A focused Input swallowing keys outside on_pre_key_down. A VirtualScrollView builder capturing a snapshot that goes stale. interactive(false) used as "disabled". A hardcoded font. A token restating what a variant already resolves.',
  },
}

const ALWAYS = ['saboteur', 'contract', 'newhire', 'minimalist']
const GATED = ['trust', 'freya']

const fileCount = Number.isInteger(a.fileCount) ? a.fileCount : null
const small = tier === 'medium' && fileCount !== null && fileCount > 0 && fileCount <= 3
const folded = tier === 'low' || small

let selected
const named = Array.isArray(a.lenses) ? a.lenses.filter(k => LENSES[k]) : []

if (Array.isArray(a.lenses) && a.lenses.length && !named.length) {
  // Nothing valid was named. Falling through to auto-selection would be fine; running
  // zero critics and reporting CLEAN would not, and that is the nearer mistake.
  log(`none of the named lenses exist (${a.lenses.join(', ')}; known: ${Object.keys(LENSES).join(', ')}) - falling back to auto-selection`)
}

if (named.length) {
  selected = named
  log(`lenses named explicitly: ${selected.join(', ')} - auto-selection skipped, and the report must say so`)
} else if (folded) {
  selected = ['folded']
  log(small
    ? `small change (${fileCount} file${fileCount === 1 ? '' : 's'}): one critic carrying every lens at ${effort} instead of the full set - proportionate to the change, still panel-verified.`
    : `low tier: one critic carrying every lens at ${effort} - still panel-verified.`)
} else {
  const all = tier === 'high' || tier === 'max'
  const fired = all ? GATED : GATED.filter(k => triggers.includes(k))
  selected = ALWAYS.concat(fired)
  const skipped = GATED.filter(k => !fired.includes(k))
  if (all) log(`${tier} tier: all ${selected.length} lenses run regardless of trigger.`)
  else if (skipped.length) log(`gated lenses not triggered by this diff, and not run: ${skipped.join(', ')}`)
}

log(`tier ${tier}: ${selected.length} critic${selected.length === 1 ? '' : 's'} at effort ${effort}, then a ${voters}-voter panel over batches of ${BATCH}${tier === 'max' ? ', then a red-team pass on the survivors' : ''}.`)

// ---------------------------------------------------------------- schemas

const FINDINGS = {
  type: 'object',
  additionalProperties: false,
  required: ['findings'],
  properties: {
    findings: {
      type: 'array',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['severity', 'file', 'line', 'short', 'claim', 'failure', 'evidence'],
        properties: {
          severity: { enum: ['CRITICAL', 'WARNING', 'NOTE'] },
          file: { type: 'string' },
          line: { type: 'integer' },
          short: { type: 'string', maxLength: 60, description: 'The claim alone, compressed to at most 60 characters for a list row: no rationale, no consequence clause. "Window > Cycle Windows dispatches a stub handler", not "This might be a problem because...".' },
          claim: { type: 'string' },
          failure: { type: 'string' },
          evidence: { type: 'string' },
        },
      },
    },
    considered: { type: 'string', description: 'When findings is empty: the strongest thing considered, and why it does not stand up.' },
  },
}

const PANEL = {
  type: 'object',
  additionalProperties: false,
  required: ['verdicts'],
  properties: {
    verdicts: {
      type: 'array',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['id', 'verdict', 'reason'],
        properties: {
          id: { type: 'integer', description: 'The candidate id you were given. Return one verdict per candidate, no more and no fewer.' },
          verdict: { enum: ['REFUTED', 'CONFIRMED'] },
          reason: { type: 'string' },
          severity: { enum: ['CRITICAL', 'WARNING', 'NOTE'] },
        },
      },
    },
  },
}

// ---------------------------------------------------------------- critique

const CONSTITUTION = `Scope under review: ${a.scope}

Read the changed files END TO END, not only the diff hunks - the defects live in the interaction
between changed and unchanged lines. Anchor every claim at file:line with the line quoted. Do not
restate what the diff does; that is not a finding. Do not hedge, and do not soften. You have no
authority to fix anything: if you want to fix it, that is a finding.

You are running in a fresh context on purpose, and you do not get the reasoning that produced this
change. That ignorance is the instrument, not a gap to fill - do not reconstruct the author's
intent, and never accept "there was probably a reason" as an answer.`

function critique(key) {
  const lens = key === 'folded'
    ? {
        title: 'Every lens at once',
        mission: 'You are the ONLY critic on this change. Nothing else will look at it.',
        hunt: Object.values(LENSES).map(l => `${l.title} - ${l.mission} ${l.hunt}`).join('\n\n'),
      }
    : LENSES[key]

  return agent(
    `${CONSTITUTION}\n\n## Your lens: ${lens.title}\n\n${lens.mission}\n\n${lens.hunt}\n\n` +
    `Report every defect you can stand behind, and rank them yourself: a panel reads all of them ` +
    `and votes each one down or through, so a weak candidate costs a fraction of one voter while a ` +
    `swallowed one ships. Do not pad. Report at most ${PER_LENS_CAP} - if you are near that, you ` +
    `are listing observations rather than defects, and the ones past it will be dropped. If you ` +
    `truly have nothing, return an empty findings array and use 'considered' to name the strongest ` +
    `thing you looked at and why it does not stand up.`,
    { label: `critic:${key}`, phase: 'Critique', agentType: 'adversarial-reviewer', schema: FINDINGS, effort },
  )
}

phase('Critique')

// A barrier, deliberately: the candidates must ALL be in hand before they can be
// deduplicated, and deduplicating before the panel is what stops six lenses billing
// six panels for one line.
const raw = (await parallel(selected.map(k => () => critique(k).then(r => ({ k, r })))))
  .filter(Boolean)

// Discovery fails closed too, not just the panel. `agent()` resolves to null when a subagent
// dies or is skipped, and an empty candidate list reads downstream exactly like a clean diff -
// so a review where every critic died would otherwise report CLEAN, which is the single worst
// thing this tool could say. A critic returning `findings: []` is a real clean result; a critic
// returning nothing at all is an absence of evidence, and the two must not collapse.
const dead = raw.filter(x => !x.r).length
if (dead === raw.length) {
  log(`every critic (${dead}/${raw.length}) returned nothing - discovery did not happen, so this is not a clean review.`)
  return {
    verdict: 'FAILED',
    error: 'discovery-failed',
    message: `All ${dead} critics died or were skipped. Nothing was reviewed. Re-run; do not read this as CLEAN.`,
    tally: { CRITICAL: 0, WARNING: 0, NOTE: 0 },
    findings: [],
    report: [],
    level: effort,
    ran: ranSummary(0, selected.length),
  }
}
if (dead) log(`${dead} of ${raw.length} critics returned nothing - their lenses did not run, and the report must say so.`)

const candidates = []
for (const { k, r } of raw) {
  if (!r) { log(`${k}: critic returned nothing (died or was skipped)`); continue }
  if (!r.findings.length) { log(`${k}: no candidates - ${r.considered || 'nothing recorded'}`); continue }
  const RANK0 = ['NOTE', 'WARNING', 'CRITICAL']
  const ranked = r.findings.slice().sort((x, y) => RANK0.indexOf(y.severity) - RANK0.indexOf(x.severity))
  if (ranked.length > PER_LENS_CAP) log(`${k}: raised ${ranked.length} candidates, keeping the ${PER_LENS_CAP} highest-severity and dropping ${ranked.length - PER_LENS_CAP}`)
  for (const f of ranked.slice(0, PER_LENS_CAP)) candidates.push({ ...f, lens: k })
}

// ---------------------------------------------------------------- dedup, then panel

const RANK = ['NOTE', 'WARNING', 'CRITICAL']

// Two lenses citing one line have not necessarily found one defect. A saboteur's "unwrap
// panics on an empty batch" and a freya pedant's "second handler registration replaces the
// first" can anchor at the same line and share nothing else - lenses hunt different things,
// so collisions at a line are routine rather than exceptional.
//
// Merging on position alone therefore does two wrong things at once: it deletes one of the
// two claims before the panel can judge it, and it then reports the collision as agreement,
// promoting a severity for a convergence that never happened. So the key is position AND
// claim: candidates at one line are clustered by what they actually assert, and a cluster
// that does not cohere stays two sites.
//
// The comparison is deliberately shallow and deterministic - overlap of content words - and
// the threshold is set to under-merge. That is the safe direction: failing to merge two
// genuinely identical claims costs one extra panel slot and one missed promotion, while
// merging two different ones destroys a finding outright and manufactures the agreement
// that promotes what is left.
const STOP = new Set(['the', 'and', 'that', 'this', 'with', 'for', 'are', 'was', 'has', 'have', 'its', 'from', 'into', 'when', 'which', 'they', 'them', 'then', 'than', 'been', 'does'])

function words(s) {
  return new Set(String(s || '').toLowerCase().replace(/[^a-z0-9_]+/g, ' ').split(' ').filter(w => w.length > 2 && !STOP.has(w)))
}

function sameDefect(x, y) {
  const A = words(`${x.claim} ${x.short || ''}`)
  const B = words(`${y.claim} ${y.short || ''}`)
  if (!A.size || !B.size) return false
  let shared = 0
  for (const w of A) if (B.has(w)) shared++
  return shared / Math.min(A.size, B.size) >= 0.5
}

const byLine = new Map()
for (const c of candidates) {
  const key = `${c.file}:${c.line}`
  if (!byLine.has(key)) byLine.set(key, [])
  byLine.get(key).push(c)
}

const sites = []
let id = 0
let merged = 0
let split = 0
for (const group of byLine.values()) {
  const clusters = []
  for (const c of group) {
    const hit = clusters.find(cl => cl.some(m => sameDefect(m, c)))
    if (hit) hit.push(c)
    else clusters.push([c])
  }
  if (group.length > 1 && clusters.length > 1) split++
  for (const cl of clusters) {
    if (cl.length > 1) merged += cl.length - 1
    const top = cl.slice().sort((x, y) => RANK.indexOf(y.severity) - RANK.indexOf(x.severity))[0]
    // `lenses` now means "lenses that reached THIS defect", which is what the promotion
    // predicate has always claimed to be reading.
    sites.push({ ...top, id: id++, lenses: Array.from(new Set(cl.map(g => g.lens))) })
  }
}

if (merged) log(`${candidates.length} candidates over ${sites.length} sites - ${merged} merged as the same defect, so an overlap is judged once.`)
if (split) log(`${split} line${split === 1 ? '' : 's'} carried more than one distinct defect and stayed separate - not treated as agreement.`)

if (!sites.length) {
  log('no candidates survived the critique stage - nothing for the panel to judge.')
  // ranSummary(0, selected.length), not (0, 0): the critics DID run, and reporting zero
  // agents on the clean path would understate a clean review into a review that never happened.
  return { verdict: 'CLEAN', tally: { CRITICAL: 0, WARNING: 0, NOTE: 0 }, findings: [], report: [], level: effort, ran: ranSummary(0, selected.length) }
}

// Three voters, three different reasons a finding dies. Identical clones are redundancy;
// distinct lenses catch failure modes redundancy cannot, at the same agent count.
const VOTER_LENS = [
  'REACHABILITY. Can the failure actually occur? Chase the input: is the state it needs constructible, is the branch reachable, does a type make it unrepresentable, does a caller already validate? Refute anything that needs a state nobody hits.',
  'CONTRACT. Does the cited rule say what the claim says it says? Open AGENTS.md and the full entry in docs/reference/ and read the actual wording. Refute any rule cited by vibe, any pre-existing behaviour the diff did not change, and any preference wearing a severity label.',
  'EVIDENCE. Does the quoted evidence establish the claim? Open the cited file at the cited line yourself. Refute anything where the quote turns out not to show what it was said to show, or where the line number does not point at what the claim describes.',
]

const batches = []
for (let i = 0; i < sites.length; i += BATCH) batches.push(sites.slice(i, i + BATCH))

log(`panel: ${voters} voter${voters === 1 ? '' : 's'} x ${batches.length} batch${batches.length === 1 ? '' : 'es'} = ${voters * batches.length} agents for ${sites.length} sites (per-candidate voting would have billed ${sites.length * voters}).`)

phase('Refute')

function ballot(batch, v) {
  const list = batch.map(f => `--- candidate ${f.id} ---\nseverity: ${f.severity}\nfile: ${f.file}:${f.line}\nclaim: ${f.claim}\nfailure: ${f.failure}\nevidence: ${f.evidence}`).join('\n\n')
  return agent(
    `Scope under review: ${a.scope}\n\n` +
    `You are one voter on a refutation panel. Below are ${batch.length} candidate findings raised ` +
    `by hostile critics under instructions to over-report, so the prior is that they are wrong: ` +
    `plausible, anchored at a real line, describing a failure that cannot actually happen.\n\n` +
    `## Your lens\n\n${VOTER_LENS[v % VOTER_LENS.length]}\n\n` +
    `Judge each candidate through that lens and return exactly one verdict per candidate id - no ` +
    `more, no fewer. Open the cited files and check for yourself; a verdict built on a candidate's ` +
    `own summary is worth nothing. Default to REFUTED when you cannot decide. Vote your own read: ` +
    `other voters carry different lenses and you cannot see them, so guessing at consensus removes ` +
    `a vote the panel needed. CONFIRM only when you can state the failure concretely - specific ` +
    `inputs or state leading to specific wrong behaviour, or the exact contract line quoted beside ` +
    `the diff line that breaks it. Adjust severity where the critic's label is wrong.\n\n` +
    `## Candidates\n\n${list}`,
    { label: `panel:v${v + 1}:${batch[0].id}-${batch[batch.length - 1].id}`, phase: 'Refute', agentType: 'finding-refuter', schema: PANEL, effort },
  )
}

const ballots = (await parallel(
  batches.flatMap((b, bi) => Array.from({ length: voters }, (_, v) => () => ballot(b, v).then(r => ({ bi, v, r })))),
)).filter(Boolean)

// Fail closed: a site whose batch lost a voter has fewer reads than the tier promised,
// and a majority computed over a short panel is not the majority that was asked for.
const returned = new Map()
for (const { bi, r } of ballots) {
  if (!r) continue
  returned.set(bi, (returned.get(bi) || 0) + 1)
}

const votes = new Map()
for (const { r } of ballots) {
  if (!r) continue
  for (const v of r.verdicts) {
    if (!votes.has(v.id)) votes.set(v.id, [])
    votes.get(v.id).push(v)
  }
}

let survivors = []
for (let bi = 0; bi < batches.length; bi++) {
  const got = returned.get(bi) || 0
  if (got < voters) {
    log(`batch ${bi + 1}: only ${got}/${voters} voters returned - its ${batches[bi].length} sites are not keepable`)
    continue
  }
  for (const site of batches[bi]) {
    const cast = votes.get(site.id) || []
    if (cast.length < voters) { log(`${site.file}:${site.line}: ${cast.length}/${voters} votes cast - not keepable`); continue }
    const yes = cast.filter(v => v.verdict === 'CONFIRMED')
    if (yes.length * 2 <= voters) continue
    survivors.push({ ...site, severity: yes[0].severity || site.severity, failure: yes[0].reason, votes: { yes: yes.length, of: voters } })
  }
}

log(`panel kept ${survivors.length} of ${sites.length} sites.`)

// Promotion happens HERE, before the red team, not after it. Independent convergence is the
// strongest signal available - which is why the critics never share a context: it only means
// something if it could have failed - but a promotion applied afterwards would silently undo
// every severity the red team had just corrected, and would hand the red team a number that
// is not the one the report prints. Promote first; let the last word belong to the pass whose
// whole job is judging whether the severity is honest.
survivors = survivors.map(s => {
  const promoted = s.lenses.length > 1 && RANK.indexOf(s.severity) < RANK.length - 1
  if (promoted) log(`${s.file}:${s.line}: reached independently by ${s.lenses.join(' + ')} - promoted to ${RANK[RANK.indexOf(s.severity) + 1]}`)
  return promoted ? { ...s, severity: RANK[RANK.indexOf(s.severity) + 1] } : s
})

// ---------------------------------------------------------------- adversarial (max only)

let redBatches = 0

if (tier === 'max' && survivors.length) {
  phase('Adversarial')
  log(`red-teaming ${survivors.length} survivor${survivors.length === 1 ? '' : 's'} on severity and reachability.`)

  const red = (await parallel(
    (() => {
      const out = []
      for (let i = 0; i < survivors.length; i += BATCH) out.push(survivors.slice(i, i + BATCH))
      redBatches = out.length
      return out
    })().map((b, bi) => () => agent(
      `Scope under review: ${a.scope}\n\n` +
      `These findings already survived a refutation panel. Do not re-litigate whether they are ` +
      `real - that is settled. Attack what is left, one verdict per id: is the severity honest or ` +
      `inflated by the drama of the failure path? Is there a simpler explanation of the same ` +
      `evidence? Is the failure reachable as often as the claim implies, or does it need a state ` +
      `nobody hits? Return CONFIRMED with a corrected severity, or REFUTED if the surviving claim ` +
      `only holds under a state that cannot occur.\n\n` +
      b.map(f => `--- candidate ${f.id} ---\nseverity: ${f.severity}\nfile: ${f.file}:${f.line}\nclaim: ${f.claim}\nfailure: ${f.failure}`).join('\n\n'),
      { label: `redteam:batch${bi + 1}`, phase: 'Adversarial', agentType: 'finding-refuter', schema: PANEL, effort },
    ))
  )).filter(Boolean)

  const verdict = new Map()
  for (const r of red) for (const v of r.verdicts) verdict.set(v.id, v)

  const before = survivors.length
  survivors = survivors
    .map(s => {
      const v = verdict.get(s.id)
      if (!v) return s                      // unjudged survives: the panel already kept it
      if (v.verdict === 'REFUTED') return null
      return { ...s, severity: v.severity || s.severity }
    })
    .filter(Boolean)
  if (survivors.length < before) log(`red team dropped ${before - survivors.length}.`)
}

// ---------------------------------------------------------------- tally

// Promotion already ran, before the red team. Nothing here may change a severity: this
// stage only orders what the earlier stages settled.
const findings = survivors.slice().sort((x, y) => RANK.indexOf(y.severity) - RANK.indexOf(x.severity))

const tally = {
  CRITICAL: findings.filter(f => f.severity === 'CRITICAL').length,
  WARNING: findings.filter(f => f.severity === 'WARNING').length,
  NOTE: findings.filter(f => f.severity === 'NOTE').length,
}

// Computed here, in code, rather than asked of a model: the same tally must always give
// the same verdict, and a synthesiser that can round a BLOCK down is not a gate.
const verdict = tally.CRITICAL > 0 ? 'BLOCK' : tally.WARNING >= 2 ? 'CONCERNS' : 'CLEAN'

log(`${verdict}: ${tally.CRITICAL} critical, ${tally.WARNING} warning, ${tally.NOTE} note.`)

function ranSummary(nSites, nAgents) {
  return {
    tier,
    effort,
    voters,
    lenses: selected,
    shape: folded ? 'one critic carrying every lens' : `${selected.length} isolated critics`,
    sites: nSites,
    agents: nAgents,
    adversarialPhase: tier === 'max',
    note: 'Report `ran` verbatim in the scope line. A tier is a claim about how hard this was looked at.',
  }
}

// Built here rather than mapped at render time, for the same reason the verdict is computed
// here: a shape the caller has to transform is a shape the caller can get wrong, and the
// findings card is the artifact most people will actually read.
const report = findings.map(f => ({
  file: f.file,
  line: f.line,
  short_summary: (f.short || f.claim).slice(0, 60),
  summary: f.claim,
  failure_scenario: f.failure,
  category: CATEGORY[f.lens] || 'correctness',
  // The panel IS the verify pass, so every reported finding carries a verdict. Unanimous
  // is CONFIRMED; a majority that one voter refused is PLAUSIBLE, and saying so is more
  // useful than flattening both to the same word.
  verdict: f.votes && f.votes.yes === f.votes.of ? 'CONFIRMED' : 'PLAUSIBLE',
}))

return {
  verdict,
  tally,
  findings,
  report,
  level: effort,
  ran: ranSummary(sites.length, selected.length + voters * batches.length + redBatches),
}
