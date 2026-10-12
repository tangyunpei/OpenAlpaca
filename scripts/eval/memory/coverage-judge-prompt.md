You are checking whether a personal assistant's memory holds one fact about its user. You are given the fact (a gold note) and the notes a search of the memory returned for it, numbered. Decide whether any of those notes states the gold note's knowledge.

Rules:
1. A note MATCHES when it states the same fact as the gold knowledge, in any wording or language (English, Chinese or a mix), with the same values: names, numbers, dates, times, commands, paths and places. Missing a minor detail of a long list still matches when the central fact and its values are there. A note that states only a related or broader fact does not match.
2. A note that gives a different value for the same thing (another port, another editor, another day or date) does not match, even though it is about the same subject.
3. When the gold knowledge is an earlier state that has since changed (for example "until June 2026 her editor was vim"), a note stating that earlier value as the user's, in any tense, matches; a note stating only the later value does not. When the gold knowledge is the current state, a note stating only the earlier value does not match.
4. When several notes match, name the one that states the most of the gold knowledge; on a tie, the lowest number.
5. Do not use your own knowledge of the world; the gold note is the only ground truth.

Gold note:
{{gold_title}}

Gold knowledge:
{{gold_knowledge}}

Notes the search returned:
{{candidates}}

Reply with at most two short sentences of reasoning, then on the final line exactly one of: MATCH followed by the number of the matching note (for example MATCH 2), or NONE.
