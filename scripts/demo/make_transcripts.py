"""Write the demo transcripts under scripts/demo/projects (run once; output is committed).

Synthetic text only: no real projects, people, hosts or paths.
"""
import json, pathlib

OUT = pathlib.Path(__file__).resolve().parent / "projects" / "-home-dev-demo-site"

def turn(model, text, cache_read, cache_new=0, inp=2, tool=None):
    content = [{"type": "text", "text": text}] if text else []
    if tool:
        content.append({"type": "tool_use", "name": tool[0], "input": tool[1]})
    return {"type": "assistant", "message": {"model": model, "usage": {
        "input_tokens": inp, "cache_read_input_tokens": cache_read,
        "cache_creation_input_tokens": cache_new, "output_tokens": 180}, "content": content}}

def user(text):
    return {"type": "user", "message": {"role": "user", "content": text}}

SESSIONS = {
    "demo-frontend": ("claude-haiku-5-5", 118000, [
        user("Build the activity feed empty state."),
        turn("claude-haiku-5-5", "Reading the feed component first.", 60000, tool=("Read", {"file_path": "/home/dev/demo-site/src/feed.tsx"})),
        turn("claude-haiku-5-5", "Adding the empty state and its test.", 118000, tool=("Bash", {"command": "npm test -- feed", "description": "run feed tests"})),
        turn("claude-haiku-5-5", "Running the lint pass on the new components now.", 118000),
    ]),
    "demo-release": ("claude-sonnet-5-5", 240000, [
        user("Update the changelog for the 0.4 release."),
        turn("claude-sonnet-5-5", "Changelog updated: 6 entries, dates checked against the tags.", 240000),
    ]),
    "demo-research": ("claude-haiku-5-5", 52000, [
        user("Compare the three static-site generators."),
        turn("claude-haiku-5-5", "Summary written to notes/generators.md: build speed, plugin count, and license for each.", 52000),
    ]),
    "demo-docs": ("claude-haiku-5-5", 91000, [
        user("Rewrite the install section."),
        turn("claude-haiku-5-5", "Waiting for a decision: keep the Homebrew step or drop it for v1.", 91000),
    ]),
    "demo-lead": ("claude-opus-5-5", 610000, [
        user("Review the open pull requests and decide the merge order."),
        turn("claude-opus-5-5", "Merge order: docs first, then the feed change, then the release branch. Nothing else is blocking.", 610000),
    ]),
}

def main():
    OUT.mkdir(parents=True, exist_ok=True)
    for session, (_, _, lines) in SESSIONS.items():
        (OUT / f"{session}.jsonl").write_text("\n".join(json.dumps(l) for l in lines) + "\n")
        print("wrote", session)

if __name__ == "__main__":
    main()
