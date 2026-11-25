# Java layer (planned)

Goal: a Sunrise client with this port joins a Java NeoForge 26.3 server running the original mod.

- Status: plan only, no code yet.
- A 26.3 original exists (26.3.2-alpha), but this port follows AE2 15.4, so ids, menus and payloads must be compared first.
- Open question: whether Sunrise's client negotiates with NeoForge servers. A text search of sunrise main found no such code. This must be confirmed by a join test first.
- Tests: join, leave and rejoin; a version mismatch gives a clear message; payloads match the original's; item counts are conserved; the server refuses malformed payloads.
