# Bounded transcript sections

Transcript routes load at most 51 timeline events at a time and render 50. The extra event only determines whether to show a next-section link. Messages, tool requests/results, and questions/answers retain source sequence order, and previous/next links preserve the session-local `section` and event-kind query parameters.

Each timeline row projects at most 2,048 Unicode characters. Longer messages, tool arguments, and results show a continuation link instead of putting the full body into every response. Continuation views read the same indexed item in 16,384-character parts and provide bounded previous/next navigation plus a link back to its anchored timeline section. Raw source files and indexed content are unchanged. See [tool and question timeline semantics](tool-timeline.md).

Grouping remains local to each section: consecutive assistant records within a section share one display block only when no tool event intervenes, while a group crossing a boundary resumes as a new block in the next section. Later sections begin with conversation context already established, preventing ordinary `You are ...` messages from being mistaken for initial configuration. Recognized tagged setup records retain their existing labels and collapsed state.

The synthetic message regression uses 106 ordered messages, including a collapsed setup message longer than 20,000 characters. It verifies three transcript sections, lazy access to both message parts, navigation boundaries, invalid inputs, HTML ordering, and responses below 128 KiB for that dataset. A separate tool-heavy regression verifies 60 events and filtered navigation. Production query/result ceilings are 51 event rows, 2,048 projected characters per timeline item, and 16,384 characters per detail-part response.

A separate 100,000-message synthetic SQLite check confirmed that the section query uses `idx_messages_session`. In one local run, fetching the 51-row result ceiling took 0.0005 seconds for section 1 and 0.0084 seconds at the section-1,000 offset; these are query-plan checks rather than general latency guarantees.

The message limits require no index change. Ordered tool results use an automatic schema upgrade and require one parser-version refresh for exact historical result order; see the timeline documentation.
