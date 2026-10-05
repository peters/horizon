---
procedure: <short-name>
feature: <feature or subsystem>
platforms: [linux]
cost: none            # none | rents compute | paid device
destructive: no       # yes if a step deletes data, credentials or resources
secrets: none         # list the secret references, never the values
owner: <GitHub user or team>
---

# <Feature> test procedure

Write this procedure in Simplified Technical English. Read
[the STE rules](../../style/ste-rules.md) and
[the technical names](../../style/technical-names.md) before you start.
Delete this paragraph in a real procedure.

## 1. Purpose

<One or two sentences. Say what this procedure proves.>

## 2. Applicability

- Candidate: <which builds this applies to>.
- Platforms: <operating systems and providers>.
- This procedure does not test: <out of scope items>.

## 3. Safety

> **CAUTION:** <COMMAND IN CAPITAL LETTERS.> <The risk in one or two sentences.>

Delete this section only if no step rents compute, deletes data, sends a secret
or changes access.

## 4. Equipment and preconditions

- <Tool or account>.
- <Credential reference. Do not write the secret.>
- <State that must exist before the first task.>

## 5. Setup

1. <One instruction.>

   Result: <What you see.>

## 6. Tasks

Give each task an ID. A report uses the ID to give a result.

### 6.1 <ID> — <Task name>

1. <One instruction.>

   Result: <What you see.>

2. <One instruction.>

   Result: <What you see.>

## 7. Pass criteria

- <Condition that must be true for the run to pass.>

## 8. Cleanup

1. <One instruction.>

   Result: <What you see.>

## 9. Record of results

Write each run as a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). Keep private evidence out of the
repository.
