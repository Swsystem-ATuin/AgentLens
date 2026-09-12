# Bounded transcript sections

Transcript routes load at most 51 message records at a time and render 50. The extra record only determines whether to show a next-section link. Sections retain database sequence order, and their previous/next links use the session-local `section` query parameter.

Each transcript row projects at most 2,048 Unicode characters of message content. Longer messages show a continuation link instead of putting the full body into every transcript response. The continuation view reads the same indexed message in 16,384-character parts and provides bounded previous/next navigation plus a link back to the section containing the message. Raw source files and indexed message content are unchanged.

Grouping remains local to each section: consecutive assistant records within a section share one display block, while a group crossing a boundary resumes as a new block in the next section. Later sections begin with conversation context already established, preventing ordinary `You are ...` messages from being mistaken for initial configuration. Recognized tagged setup records retain their existing labels and collapsed state.

The synthetic regression uses 106 ordered messages, including a collapsed setup message longer than 20,000 characters. It verifies three transcript sections, lazy access to both message parts, navigation boundaries, invalid inputs, HTML ordering, and responses below 128 KiB for that dataset. Production query/result ceilings are 51 message rows, 2,048 projected characters per transcript row, and 16,384 characters per message-part response.

A separate 100,000-message synthetic SQLite check confirmed that the section query uses `idx_messages_session`. In one local run, fetching the 51-row result ceiling took 0.0005 seconds for section 1 and 0.0084 seconds at the section-1,000 offset; these are query-plan checks rather than general latency guarantees.

This is a presentation and query change. Existing indexes require neither refresh nor rebuild; restart the server after updating.
