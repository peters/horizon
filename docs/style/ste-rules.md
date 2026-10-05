# Simplified Technical English rules for Horizon documents

Horizon writes test procedures, setup guides and runbooks in ASD-STE100
Simplified Technical English (STE). This file gives the rules that apply in this
repository. It does not replace the specification. The free specification is
available from [asd-ste100.org](https://www.asd-ste100.org/).

## Scope

STE is mandatory for these documents:

- Test procedures in `docs/testing/procedures/`.
- Test reports in `docs/testing/reports/` (descriptive rules only).
- Setup guides and runbooks, for example `scripts/device-smoke/README.md`.
- Procedure sections in `AGENTS.md`.

STE is recommended for reference and architecture documents. Do not convert
historical documents. Move them to `docs/archive/` instead.

## Words

1. Use only approved words or technical names. Use each word with one meaning
   and one part of speech.
2. Use the same word for the same thing every time. Use the names in
   [technical-names.md](technical-names.md).
3. Do not use a word from the left column. Use the word in the right column.

   | Do not use | Use |
   |---|---|
   | ensure, verify (as an instruction) | make sure, examine |
   | confirm (as an instruction) | make sure |
   | utilize | use |
   | commence, initiate | start |
   | prior to | before |
   | approximately | about |
   | in order to | to |
   | via | through, with |
   | replenish | fill |
   | terminate | stop |
   | e.g., i.e. | for example, that is |
   | obtain, acquire | get |
   | sufficient | enough |

4. Do not leave out "the", "a" or "this".
5. A technical verb is permitted when it names an operation in Horizon or a
   tool. Examples: "deploy", "provision", "squash-merge", "rebase". Write it in
   the glossary before you use it.

## Noun clusters

- Use a maximum of three words in a noun cluster.
- If a technical name is longer, write it in full one time. Then use a short
  name from the glossary.

## Verbs

- Use these forms only: the imperative, the simple present, the simple past,
  the simple future, the infinitive, and the past participle as an adjective.
- Do not use the -ing form as a verb. You can use it in a technical name, for
  example "Local Network Bridge settings".
- Do not use perfect tenses. Write "The worker stopped", not "The worker has
  stopped".
- Use the active voice in procedures. Use the passive voice in descriptive
  text only when the agent is not known or not important.

## Sentences

| Sentence type | Maximum |
|---|---|
| Procedural sentence | 20 words |
| Descriptive sentence | 25 words |
| Paragraph | 6 sentences |
| Instructions in one sentence | 1 (2 only for simultaneous actions) |

Count a command, a path or a UI label as one word.

## Procedures

1. Write each step as a command.
2. Write one instruction in each step.
3. If a condition applies, write the condition first. Example: "If the card
   shows Ready, click Stop worker".
4. Write the expected result on a separate line that starts with `Result:`.
5. Use a vertical list for three or more items.
6. Put a command that you must type in a code block.

## Safety instructions

Put a safety instruction before the step that it applies to.

- **WARNING** tells the reader about a risk of injury.
- **CAUTION** tells the reader about a risk of damage, data loss, loss of
  access or cost.

Write the instruction as follows:

1. Start with the word `WARNING:` or `CAUTION:`.
2. Then write a short, clear command in capital letters.
3. Then write the risk in one or two short sentences.

Example:

> **CAUTION:** DELETE ONLY THE WORKERS THAT THIS RUN RECORDED. If you delete
> other workers, other people lose their work.

Most Horizon procedures need a CAUTION in these conditions:

- The step rents compute or a paid device.
- The step deletes a cloud, a volume, a credential or a session.
- The step sends a secret, for example an API key or an auth key.
- The step changes access, for example a tailnet or a firewall rule.
- The step publishes something that you cannot undo, for example a release.

## Descriptive text

- Write one topic in each paragraph.
- Start a paragraph with the topic sentence.
- Use the simple present tense for facts.

## Check list for a reviewer

- [ ] Each step has one instruction and 20 words or fewer.
- [ ] Each step has a `Result:` line, or the result is clear from the next step.
- [ ] Each paid, destructive or secret step has a CAUTION before it.
- [ ] The document uses only the names in the glossary.
- [ ] There are no words from the "Do not use" column.
- [ ] There are no -ing verb forms, perfect tenses or passive steps.
