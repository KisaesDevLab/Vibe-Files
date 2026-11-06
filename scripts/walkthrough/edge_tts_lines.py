"""Speaks each line of a narration JSON file ([{name, say}]) into <out_dir>/<name>.mp3 with a
Microsoft Edge neural voice (the `edge-tts` package: the text goes to Microsoft's read-aloud
service, the audio comes back). Marketing copy only; never client data. Called by
..\\walkthrough-video.ps1.

    python edge_tts_lines.py narration.json out_dir [voice] [rate]
"""

import asyncio
import json
import pathlib
import sys

import edge_tts


async def main() -> None:
    lines = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
    out = pathlib.Path(sys.argv[2])
    voice = sys.argv[3] if len(sys.argv) > 3 else "en-US-AndrewNeural"
    rate = sys.argv[4] if len(sys.argv) > 4 else "-5%"
    for line in lines:
        path = out / f"{line['name']}.mp3"
        # The service now and then answers with no audio; ask again.
        for attempt in range(1, 6):
            try:
                await edge_tts.Communicate(line["say"], voice, rate=rate).save(str(path))
                break
            except edge_tts.exceptions.NoAudioReceived:
                if attempt == 5:
                    raise
                print(f"{line['name']}: no audio, trying again", file=sys.stderr)
                await asyncio.sleep(2 * attempt)


asyncio.run(main())
