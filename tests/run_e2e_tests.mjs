#!/usr/bin/env node
/**
 * Comprehensive End-to-End Test Suite for Audio Transcription Studio (MVP v1)
 * Runs against the real compiled release binary and real whisper.cpp + ggml-base.bin.
 */

import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

const PROJECT_ROOT = path.resolve(process.cwd());
const RELEASE_BIN = path.join(
  PROJECT_ROOT,
  "src-tauri/target/release/audio-transcription-service"
);
const FIXTURES_DIR = path.join(PROJECT_ROOT, "tests/fixtures");
const TEST_PORT = 4995;
const API_BASE = `http://127.0.0.1:${TEST_PORT}`;
const REAL_SAMPLE_MP3 = "/Users/alekseykhokhlov/repo/golos/audio.mp3";

function runCmd(cmd, args) {
  return execFileSync(cmd, args, { stdio: "pipe" }).toString("utf8");
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function generateSpeechWav(voice, text, outWav) {
  const aiff = `${outWav}.aiff`;
  runCmd("say", ["-v", voice, "-o", aiff, text]);
  runCmd("ffmpeg", [
    "-y",
    "-v",
    "error",
    "-i",
    aiff,
    "-ar",
    "16000",
    "-ac",
    "1",
    "-c:a",
    "pcm_s16le",
    outWav,
  ]);
  fs.rmSync(aiff, { force: true });
}

function generateFixtures() {
  fs.rmSync(FIXTURES_DIR, { recursive: true, force: true });
  fs.mkdirSync(FIXTURES_DIR, { recursive: true });

  console.log("Generating realistic speech, video, noise, silence, and long-duration fixtures...");

  // 1. Format fixtures: WAV, MP3, M4A, MP4 (with video stream)
  const fmtWav = path.join(FIXTURES_DIR, "format_en.wav");
  generateSpeechWav(
    "Daniel",
    "Audio Transcription Studio supports WAV, MP3, M4A, and MP4 formats locally.",
    fmtWav
  );
  runCmd("ffmpeg", [
    "-y",
    "-v",
    "error",
    "-i",
    fmtWav,
    "-c:a",
    "libmp3lame",
    "-b:a",
    "128k",
    path.join(FIXTURES_DIR, "format_en.mp3"),
  ]);
  runCmd("ffmpeg", [
    "-y",
    "-v",
    "error",
    "-i",
    fmtWav,
    "-c:a",
    "aac",
    "-b:a",
    "128k",
    path.join(FIXTURES_DIR, "format_en.m4a"),
  ]);
  runCmd("ffmpeg", [
    "-y",
    "-v",
    "error",
    "-f",
    "lavfi",
    "-i",
    "color=c=0x111827:s=320x240:r=5",
    "-i",
    fmtWav,
    "-shortest",
    "-c:v",
    "libx264",
    "-tune",
    "stillimage",
    "-c:a",
    "aac",
    "-b:a",
    "128k",
    path.join(FIXTURES_DIR, "format_en.mp4"),
  ]);

  // Real-world Russian MP3 from /Users/alekseykhokhlov/repo/golos/audio.mp3 if present
  if (fs.existsSync(REAL_SAMPLE_MP3)) {
    fs.copyFileSync(REAL_SAMPLE_MP3, path.join(FIXTURES_DIR, "real_sample.mp3"));
  }

  // 2. Language fixtures: EN, RU, Mixed (RU + EN)
  generateSpeechWav(
    "Daniel",
    "Good morning. This desktop application transcribes audio offline without API keys.",
    path.join(FIXTURES_DIR, "lang_en.wav")
  );
  const ruWav = path.join(FIXTURES_DIR, "lang_ru.wav");
  generateSpeechWav(
    "Milena",
    "Добрый день. Локальная студия автоматически распознаёт русскую речь и сохраняет субтитры.",
    ruWav
  );
  const enPartWav = path.join(FIXTURES_DIR, "lang_mixed_en_part.wav");
  generateSpeechWav(
    "Daniel",
    "Now we switch to English to confirm bilingual subtitles and accurate timings.",
    enPartWav
  );
  runCmd("ffmpeg", [
    "-y",
    "-v",
    "error",
    "-i",
    ruWav,
    "-f",
    "lavfi",
    "-t",
    "0.8",
    "-i",
    "anullsrc=r=16000:cl=mono",
    "-i",
    enPartWav,
    "-filter_complex",
    "[0:a][1:a][2:a]concat=n=3:v=0:a=1[a]",
    "-map",
    "[a]",
    "-ar",
    "16000",
    "-ac",
    "1",
    "-c:a",
    "pcm_s16le",
    path.join(FIXTURES_DIR, "lang_mixed.wav"),
  ]);
  fs.rmSync(enPartWav, { force: true });

  // 3. Duration fixtures: 10 seconds, 5 minutes, 1 hour
  const durSpeech1 = path.join(FIXTURES_DIR, "dur_s1.wav");
  const durSpeech2 = path.join(FIXTURES_DIR, "dur_s2.wav");
  const durSpeech3 = path.join(FIXTURES_DIR, "dur_s3.wav");
  generateSpeechWav("Daniel", "Beginning checkpoint at two seconds.", durSpeech1);
  generateSpeechWav("Daniel", "Middle checkpoint reached cleanly.", durSpeech2);
  generateSpeechWav("Daniel", "Final checkpoint near the end of the recording.", durSpeech3);

  // 10s clip: 2s silence + speech1 + pad to 10s
  runCmd("ffmpeg", [
    "-y",
    "-v",
    "error",
    "-f",
    "lavfi",
    "-t",
    "2",
    "-i",
    "anullsrc=r=16000:cl=mono",
    "-i",
    durSpeech1,
    "-f",
    "lavfi",
    "-t",
    "6",
    "-i",
    "anullsrc=r=16000:cl=mono",
    "-filter_complex",
    "[0:a][1:a][2:a]concat=n=3:v=0:a=1[a]",
    "-map",
    "[a]",
    "-t",
    "10",
    "-ar",
    "16000",
    "-ac",
    "1",
    path.join(FIXTURES_DIR, "dur_10s.wav"),
  ]);

  // Helper to build long exact-duration audio using silence blocks + 3 speech cues
  const buildLongAudio = (outMp3, silenceMidSec, silenceEndSec) => {
    const sil2 = path.join(FIXTURES_DIR, "sil2.wav");
    const silMid = path.join(FIXTURES_DIR, `sil_${silenceMidSec}.wav`);
    const silEnd = path.join(FIXTURES_DIR, `sil_${silenceEndSec}.wav`);
    runCmd("ffmpeg", [
      "-y",
      "-v",
      "error",
      "-f",
      "lavfi",
      "-t",
      "2",
      "-i",
      "anullsrc=r=16000:cl=mono",
      sil2,
    ]);
    runCmd("ffmpeg", [
      "-y",
      "-v",
      "error",
      "-f",
      "lavfi",
      "-t",
      String(silenceMidSec),
      "-i",
      "anullsrc=r=16000:cl=mono",
      silMid,
    ]);
    runCmd("ffmpeg", [
      "-y",
      "-v",
      "error",
      "-f",
      "lavfi",
      "-t",
      String(silenceEndSec),
      "-i",
      "anullsrc=r=16000:cl=mono",
      silEnd,
    ]);

    const listPath = path.join(FIXTURES_DIR, `concat_${silenceMidSec}.txt`);
    fs.writeFileSync(
      listPath,
      [
        `file '${sil2}'`,
        `file '${durSpeech1}'`,
        `file '${silMid}'`,
        `file '${durSpeech2}'`,
        `file '${silEnd}'`,
        `file '${durSpeech3}'`,
        `file '${sil2}'`,
      ].join("\n")
    );
    runCmd("ffmpeg", [
      "-y",
      "-v",
      "error",
      "-f",
      "concat",
      "-safe",
      "0",
      "-i",
      listPath,
      "-c:a",
      "libmp3lame",
      "-b:a",
      "64k",
      outMp3,
    ]);
    fs.rmSync(sil2, { force: true });
    fs.rmSync(silMid, { force: true });
    fs.rmSync(silEnd, { force: true });
    fs.rmSync(listPath, { force: true });
  };

  // 5 minutes (~300s): speech at ~2s, ~150s (2m30s), ~290s (4m50s)
  buildLongAudio(path.join(FIXTURES_DIR, "dur_5m.mp3"), 145, 138);
  // 1 hour (~3600s): speech at ~2s, ~1800s (30m00s), ~3585s (59m45s)
  buildLongAudio(path.join(FIXTURES_DIR, "dur_1h.mp3"), 1795, 1782);

  fs.rmSync(durSpeech1, { force: true });
  fs.rmSync(durSpeech2, { force: true });
  fs.rmSync(durSpeech3, { force: true });

  // 4. Audio quality fixtures: noisy speech, multi-speaker dialogue, pure silence
  const cleanForNoise = path.join(FIXTURES_DIR, "clean_noise.wav");
  generateSpeechWav(
    "Daniel",
    "Testing speech recognition under steady background noise conditions.",
    cleanForNoise
  );
  runCmd("ffmpeg", [
    "-y",
    "-v",
    "error",
    "-i",
    cleanForNoise,
    "-f",
    "lavfi",
    "-i",
    "anoisesrc=color=pink:r=16000:a=0.035",
    "-filter_complex",
    "[0:a][1:a]amix=inputs=2:duration=first:dropout_transition=0[a]",
    "-map",
    "[a]",
    "-ar",
    "16000",
    "-ac",
    "1",
    path.join(FIXTURES_DIR, "quality_noise.wav"),
  ]);
  fs.rmSync(cleanForNoise, { force: true });

  const spk1 = path.join(FIXTURES_DIR, "spk1.wav");
  const spk2 = path.join(FIXTURES_DIR, "spk2.wav");
  const spk3 = path.join(FIXTURES_DIR, "spk3.wav");
  generateSpeechWav("Daniel", "Hello team, please review the quarterly budget report.", spk1);
  generateSpeechWav("Kathy", "Confirmed, I already checked the expenses and summary.", spk2);
  generateSpeechWav("Ralph", "Great, export the subtitles before our afternoon sync.", spk3);
  runCmd("ffmpeg", [
    "-y",
    "-v",
    "error",
    "-i",
    spk1,
    "-f",
    "lavfi",
    "-t",
    "0.8",
    "-i",
    "anullsrc=r=16000:cl=mono",
    "-i",
    spk2,
    "-f",
    "lavfi",
    "-t",
    "0.8",
    "-i",
    "anullsrc=r=16000:cl=mono",
    "-i",
    spk3,
    "-filter_complex",
    "[0:a][1:a][2:a][3:a][4:a]concat=n=5:v=0:a=1[a]",
    "-map",
    "[a]",
    "-ar",
    "16000",
    "-ac",
    "1",
    path.join(FIXTURES_DIR, "quality_multispeaker.wav"),
  ]);
  fs.rmSync(spk1, { force: true });
  fs.rmSync(spk2, { force: true });
  fs.rmSync(spk3, { force: true });

  runCmd("ffmpeg", [
    "-y",
    "-v",
    "error",
    "-f",
    "lavfi",
    "-t",
    "10",
    "-i",
    "anullsrc=r=16000:cl=mono",
    path.join(FIXTURES_DIR, "quality_silence.wav"),
  ]);

  // 5. Error fixtures: corrupted MP3 & unsupported extension
  fs.writeFileSync(
    path.join(FIXTURES_DIR, "corrupted.mp3"),
    Buffer.from("NOT_A_VALID_MP3_STREAM_TRUNCATED_GARBAGE_BYTES_0123456789")
  );
  fs.writeFileSync(path.join(FIXTURES_DIR, "unsupported.xyz"), "plain text not audio");
}

function launchApp(stateDir, port = TEST_PORT) {
  const child = spawn(RELEASE_BIN, [], {
    env: {
      ...process.env,
      AUDIO_STUDIO_STATE_DIR: stateDir,
      AUDIO_STUDIO_PORT: String(port),
    },
    stdio: "ignore",
  });
  return child;
}

async function waitForHealth(base = API_BASE, timeoutMs = 15000) {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    try {
      const res = await fetch(`${base}/health`);
      if (res.ok) {
        return await res.json();
      }
    } catch {
      // retry
    }
    await sleep(200);
  }
  throw new Error(`App HTTP API on ${base} did not become healthy within ${timeoutMs}ms`);
}

async function apiCommand(action, payload = {}, base = API_BASE) {
  const res = await fetch(`${base}/command`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ action, payload }),
  });
  const data = await res.json();
  if (!res.ok) {
    throw new Error(`Command ${action} failed (${res.status}): ${JSON.stringify(data)}`);
  }
  return data;
}

async function getState(base = API_BASE) {
  const res = await fetch(`${base}/state`);
  return await res.json();
}

async function waitForItemTerminal(itemId, timeoutMs = 120000, base = API_BASE) {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    const state = await getState(base);
    const item = state.items.find((i) => i.id === itemId);
    if (
      item &&
      ["completed", "failed", "skippedDuplicate"].includes(item.status)
    ) {
      return item;
    }
    await sleep(250);
  }
  throw new Error(`Timed out waiting for item ${itemId}`);
}

const results = [];
async function runTest(name, fn) {
  const t0 = Date.now();
  try {
    const detail = await fn();
    const ms = Date.now() - t0;
    results.push({ name, status: "PASS", ms, detail });
    console.log(`✅ PASS | ${name} (${ms}ms) -> ${detail}`);
  } catch (err) {
    const ms = Date.now() - t0;
    results.push({ name, status: "FAIL", ms, detail: String(err.stack || err) });
    console.error(`❌ FAIL | ${name} (${ms}ms) -> ${err}`);
  }
}

async function main() {
  assert.ok(fs.existsSync(RELEASE_BIN), `Missing compiled release binary: ${RELEASE_BIN}`);
  generateFixtures();

  const mainStateDir = path.join(FIXTURES_DIR, "state_main");
  fs.mkdirSync(mainStateDir, { recursive: true });
  let appProc = launchApp(mainStateDir, TEST_PORT);

  const cleanup = () => {
    try {
      appProc?.kill("SIGKILL");
    } catch {}
  };
  process.on("exit", cleanup);

  try {
    const health = await waitForHealth();
    assert.equal(health.ok, true);
    assert.equal(health.engine.ready, true, `Engine not ready: ${JSON.stringify(health.engine)}`);
    assert.equal(health.engine.offlineMode, true);
    console.log(
      `Engine ready: ${health.engine.whisperBin} | Model: ${health.engine.modelName}`
    );

    // 1. Formats: MP3, WAV, M4A, MP4 (+ real sample MP3)
    await runTest("1. Formats: MP3, WAV, M4A, MP4 + TXT/SRT/VTT exports", async () => {
      const files = [
        path.join(FIXTURES_DIR, "format_en.wav"),
        path.join(FIXTURES_DIR, "format_en.mp3"),
        path.join(FIXTURES_DIR, "format_en.m4a"),
        path.join(FIXTURES_DIR, "format_en.mp4"),
      ];
      // Make unique per format so duplicate SHA-256 guard doesn't skip them!
      // Wait: format_en.wav/mp3/m4a/mp4 have different container headers/bytes so SHA-256 differs!
      const res = await apiCommand("enqueue_files", { paths: files });
      assert.equal(res.items.length, 4);

      const completed = [];
      for (const it of res.items) {
        const done = await waitForItemTerminal(it.id, 90000);
        assert.equal(done.status, "completed", `${done.fileName} failed: ${done.errorMessage}`);
        assert.ok(done.txtPath && fs.existsSync(done.txtPath));
        assert.ok(done.srtPath && fs.existsSync(done.srtPath));
        assert.ok(done.vttPath && fs.existsSync(done.vttPath));
        const txt = fs.readFileSync(done.txtPath, "utf8");
        const srt = fs.readFileSync(done.srtPath, "utf8");
        const vtt = fs.readFileSync(done.vttPath, "utf8");
        assert.match(txt.toLowerCase(), /audio|transcription|studio/);
        assert.match(srt, /\d{2}:\d{2}:\d{2},\d{3} --> \d{2}:\d{2}:\d{2},\d{3}/);
        assert.ok(vtt.startsWith("WEBVTT\n\n"));
        completed.push(`${done.format}:${done.segments.length}segs`);
      }

      if (fs.existsSync(path.join(FIXTURES_DIR, "real_sample.mp3"))) {
        const r = await apiCommand("enqueue_files", {
          paths: [path.join(FIXTURES_DIR, "real_sample.mp3")],
        });
        const doneReal = await waitForItemTerminal(r.items[0].id, 90000);
        assert.equal(doneReal.status, "completed");
        assert.ok(fs.readFileSync(doneReal.txtPath, "utf8").trim().length > 10);
        completed.push(`real_mp3:${doneReal.detectedLanguage}`);
      }
      return completed.join(", ");
    });

    // 2. Languages: English, Russian, Mixed (RU + EN)
    await runTest("2. Languages: English, Russian, Mixed RU+EN", async () => {
      const enRes = await apiCommand("enqueue_files", {
        paths: [path.join(FIXTURES_DIR, "lang_en.wav")],
      });
      const ruRes = await apiCommand("enqueue_files", {
        paths: [path.join(FIXTURES_DIR, "lang_ru.wav")],
      });
      const mixRes = await apiCommand("enqueue_files", {
        paths: [path.join(FIXTURES_DIR, "lang_mixed.wav")],
      });

      const enDone = await waitForItemTerminal(enRes.items[0].id);
      const ruDone = await waitForItemTerminal(ruRes.items[0].id);
      const mixDone = await waitForItemTerminal(mixRes.items[0].id);

      assert.equal(enDone.status, "completed");
      assert.equal(enDone.detectedLanguage, "en");
      assert.match(fs.readFileSync(enDone.txtPath, "utf8").toLowerCase(), /offline|audio|desktop/);

      assert.equal(ruDone.status, "completed");
      assert.equal(ruDone.detectedLanguage, "ru");
      assert.match(fs.readFileSync(ruDone.txtPath, "utf8").toLowerCase(), /речь|студи|субтитр|день/);

      assert.equal(mixDone.status, "completed");
      assert.equal(mixDone.detectedLanguage, "mixed (ru+en)");
      const mixTxt = fs.readFileSync(mixDone.txtPath, "utf8");
      assert.match(mixTxt, /[А-Яа-яЁё]{3,}/);
      assert.match(mixTxt, /[A-Za-z]{3,}/);

      return `en="${enDone.detectedLanguage}", ru="${ruDone.detectedLanguage}", mixed="${mixDone.detectedLanguage}"`;
    });

    // 3. Durations: 10s, 5m, 1h
    await runTest("3. Durations: 10 seconds, 5 minutes, 1 hour", async () => {
      const d10 = await apiCommand("enqueue_files", {
        paths: [path.join(FIXTURES_DIR, "dur_10s.wav")],
      });
      const d5m = await apiCommand("enqueue_files", {
        paths: [path.join(FIXTURES_DIR, "dur_5m.mp3")],
      });
      const d1h = await apiCommand("enqueue_files", {
        paths: [path.join(FIXTURES_DIR, "dur_1h.mp3")],
      });

      const done10 = await waitForItemTerminal(d10.items[0].id, 60000);
      const done5m = await waitForItemTerminal(d5m.items[0].id, 120000);
      const done1h = await waitForItemTerminal(d1h.items[0].id, 180000);

      assert.equal(done10.status, "completed");
      assert.ok(done10.durationMs >= 9500 && done10.durationMs <= 11000);

      assert.equal(done5m.status, "completed");
      assert.ok(done5m.durationMs >= 290000, `5m durationMs=${done5m.durationMs}`);
      const srt5m = fs.readFileSync(done5m.srtPath, "utf8");
      assert.match(srt5m, /00:04:/, "5m SRT should reach 4+ minutes");

      assert.equal(done1h.status, "completed");
      assert.ok(done1h.durationMs >= 3580000, `1h durationMs=${done1h.durationMs}`);
      const srt1h = fs.readFileSync(done1h.srtPath, "utf8");
      assert.match(srt1h, /00:30:/, "1h SRT should contain 30-minute checkpoint");
      assert.match(srt1h, /00:59:/, "1h SRT should contain 59-minute checkpoint");

      return `10s=${done10.durationMs}ms (${done10.segments.length}segs), 5m=${done5m.durationMs}ms (${done5m.segments.length}segs), 1h=${done1h.durationMs}ms (${done1h.segments.length}segs)`;
    });

    // 4. Audio Quality: background noise, multiple speakers, pure silence
    await runTest("4. Audio Quality: noise, multi-speaker, silence", async () => {
      const qNoise = await apiCommand("enqueue_files", {
        paths: [path.join(FIXTURES_DIR, "quality_noise.wav")],
      });
      const qMulti = await apiCommand("enqueue_files", {
        paths: [path.join(FIXTURES_DIR, "quality_multispeaker.wav")],
      });
      const qSil = await apiCommand("enqueue_files", {
        paths: [path.join(FIXTURES_DIR, "quality_silence.wav")],
      });

      const dNoise = await waitForItemTerminal(qNoise.items[0].id);
      const dMulti = await waitForItemTerminal(qMulti.items[0].id);
      const dSil = await waitForItemTerminal(qSil.items[0].id);

      assert.equal(dNoise.status, "completed");
      assert.match(
        fs.readFileSync(dNoise.txtPath, "utf8").toLowerCase(),
        /speech|recognition|noise|background/
      );

      assert.equal(dMulti.status, "completed");
      assert.ok(dMulti.segments.length >= 2, `Expected >=2 dialogue segments, got ${dMulti.segments.length}`);
      const multiTxt = fs.readFileSync(dMulti.txtPath, "utf8").toLowerCase();
      assert.match(multiTxt, /team|budget|report|expenses|subtitles/);

      assert.equal(dSil.status, "completed");
      assert.equal(dSil.isSilent, true);
      assert.equal(dSil.detectedLanguage, "silence");
      assert.equal(dSil.segments.length, 0);
      assert.equal(fs.readFileSync(dSil.txtPath, "utf8"), "");
      assert.equal(fs.readFileSync(dSil.vttPath, "utf8"), "WEBVTT\n\n");

      return `noise="${fs.readFileSync(dNoise.txtPath, "utf8").trim()}", multi=${dMulti.segments.length}segs, silence=0 hallucinations`;
    });

    // 5. Watch Folder: 10 files, slow copy completion wait, no duplicates
    await runTest("5. Watch Folder: 10 files + slow copy stability + zero duplicates", async () => {
      const watchDir = path.join(FIXTURES_DIR, "watch_dir");
      fs.mkdirSync(watchDir, { recursive: true });
      await apiCommand("set_watch_folder", { folder: watchDir });

      // Generate 9 distinct speech WAVs + 1 slow-copy MP3 written in 4 chunks
      for (let i = 1; i <= 9; i++) {
        generateSpeechWav(
          "Daniel",
          `Watch folder file number ${i} transcribed automatically.`,
          path.join(watchDir, `watch_${String(i).padStart(2, "0")}.wav`)
        );
      }

      const slowSrc = path.join(FIXTURES_DIR, "slow_source.mp3");
      const slowTmpWav = path.join(FIXTURES_DIR, "slow_tmp.wav");
      generateSpeechWav(
        "Daniel",
        "Slow copy completion verification sentence at the very end.",
        slowTmpWav
      );
      runCmd("ffmpeg", [
        "-y",
        "-v",
        "error",
        "-i",
        slowTmpWav,
        "-c:a",
        "libmp3lame",
        "-b:a",
        "128k",
        slowSrc,
      ]);
      fs.rmSync(slowTmpWav, { force: true });

      const fullBuf = fs.readFileSync(slowSrc);
      const slowDest = path.join(watchDir, "watch_10_slow.mp3");
      const chunkSize = Math.ceil(fullBuf.length / 4);
      const fd = fs.openSync(slowDest, "w");
      for (let offset = 0; offset < fullBuf.length; offset += chunkSize) {
        const slice = fullBuf.subarray(offset, Math.min(offset + chunkSize, fullBuf.length));
        fs.writeSync(fd, slice);
        fs.fsyncSync(fd);
        await sleep(350);
      }
      fs.closeSync(fd);

      // Wait until all 10 files in watch_dir have completed
      const start = Date.now();
      let watchItems = [];
      while (Date.now() - start < 90000) {
        const state = await getState();
        watchItems = state.items.filter(
          (i) => i.source === "watch" && i.filePath.startsWith(fs.realpathSync(watchDir))
        );
        const doneCount = watchItems.filter((i) => i.status === "completed").length;
        if (doneCount === 10) break;
        await sleep(300);
      }

      assert.equal(watchItems.length, 10, `Expected 10 watch items, got ${watchItems.length}`);
      for (let i = 1; i <= 9; i++) {
        const base = path.join(watchDir, `watch_${String(i).padStart(2, "0")}`);
        assert.ok(fs.existsSync(`${base}.txt`));
        assert.ok(fs.existsSync(`${base}.srt`));
        assert.ok(fs.existsSync(`${base}.vtt`));
      }
      const slowTxt = fs.readFileSync(path.join(watchDir, "watch_10_slow.txt"), "utf8").toLowerCase();
      assert.match(slowTxt, /very end|verification|sentence/);

      // Wait 1.5s more to verify no duplicates are created on subsequent polling ticks
      await sleep(1500);
      const stateAfter = await getState();
      const watchItemsAfter = stateAfter.items.filter(
        (i) => i.source === "watch" && i.filePath.startsWith(fs.realpathSync(watchDir))
      );
      assert.equal(watchItemsAfter.length, 10);

      await apiCommand("stop_watch_folder");
      return `10/10 files transcribed (30 export files created), slow copy complete, 0 duplicates`;
    });

    // 6. Duplicate protection
    await runTest("6. Duplicate Protection: same path and identical SHA-256 copy", async () => {
      const orig = path.join(FIXTURES_DIR, "lang_en.wav");
      const dupCopy = path.join(FIXTURES_DIR, "lang_en_duplicate_copy.wav");
      fs.copyFileSync(orig, dupCopy);

      const r1 = await apiCommand("enqueue_files", { paths: [orig] });
      const r2 = await apiCommand("enqueue_files", { paths: [dupCopy] });

      assert.equal(r1.items[0].status, "skippedDuplicate");
      assert.equal(r2.items[0].status, "skippedDuplicate");
      return `Both re-submissions marked skippedDuplicate without re-running Whisper`;
    });

    // 7. Error handling: corrupted file, unsupported format, permission denied
    await runTest("7. Errors: corrupted file, unsupported format, permission denied", async () => {
      await apiCommand("clear_errors");

      const badMp3 = await apiCommand("enqueue_files", {
        paths: [path.join(FIXTURES_DIR, "corrupted.mp3")],
      });
      const badExt = await apiCommand("enqueue_files", {
        paths: [path.join(FIXTURES_DIR, "unsupported.xyz")],
      });

      const roDir = path.join(FIXTURES_DIR, "readonly_dir");
      fs.mkdirSync(roDir, { recursive: true });
      const roWav = path.join(roDir, "ro_test.wav");
      fs.copyFileSync(path.join(FIXTURES_DIR, "lang_en.wav"), roWav);
      // Force enqueue because SHA matches lang_en.wav; we want to hit export permission denied!
      fs.chmodSync(roDir, 0o555);
      const roRes = await apiCommand("enqueue_files", { paths: [roWav], force: true });

      const doneBadMp3 = await waitForItemTerminal(badMp3.items[0].id);
      const doneBadExt = await waitForItemTerminal(badExt.items[0].id);
      const doneRo = await waitForItemTerminal(roRes.items[0].id);

      // Restore permissions immediately for cleanup
      fs.chmodSync(roDir, 0o755);

      assert.equal(doneBadMp3.status, "failed");
      assert.equal(doneBadMp3.errorCode, "CORRUPTED_FILE");
      assert.equal(doneBadExt.status, "failed");
      assert.equal(doneBadExt.errorCode, "UNSUPPORTED_FORMAT");
      assert.equal(doneRo.status, "failed");
      assert.equal(doneRo.errorCode, "PERMISSION_DENIED");

      // Also test inaccessible watch folder
      const noAccessWatch = path.join(FIXTURES_DIR, "no_access_watch");
      fs.mkdirSync(noAccessWatch, { recursive: true });
      fs.chmodSync(noAccessWatch, 0o000);
      let watchDenied = false;
      try {
        await apiCommand("set_watch_folder", { folder: noAccessWatch });
      } catch {
        watchDenied = true;
      } finally {
        fs.chmodSync(noAccessWatch, 0o755);
      }
      assert.equal(watchDenied, true);

      const state = await getState();
      const codes = state.errorLog.map((e) => e.errorCode);
      assert.ok(codes.includes("CORRUPTED_FILE"));
      assert.ok(codes.includes("UNSUPPORTED_FORMAT"));
      assert.ok(codes.includes("PERMISSION_DENIED"));
      return `Error log captured: ${[...new Set(codes)].join(", ")}`;
    });

    // 8. Reliability: kill app mid-queue & restart -> resumes without losing files
    await runTest("8. Reliability: kill mid-queue & restart resumes remaining files", async () => {
      const relDir = path.join(FIXTURES_DIR, "reliability");
      fs.mkdirSync(relDir, { recursive: true });
      const relFiles = [];
      for (let i = 1; i <= 4; i++) {
        const p = path.join(relDir, `rel_${i}.wav`);
        generateSpeechWav(
          "Daniel",
          `Reliability persistence test item number ${i} verifying queue recovery.`,
          p
        );
        relFiles.push(p);
      }

      const enq = await apiCommand("enqueue_files", { paths: relFiles });
      const ids = enq.items.map((x) => x.id);
      await sleep(300);

      // Kill app abruptly
      appProc.kill("SIGKILL");
      await sleep(500);

      // Restart with same state directory
      appProc = launchApp(mainStateDir, TEST_PORT);
      await waitForHealth();

      for (const id of ids) {
        const done = await waitForItemTerminal(id, 90000);
        assert.equal(done.status, "completed", `${done.fileName} failed after restart`);
        assert.ok(fs.existsSync(done.txtPath));
        assert.ok(fs.existsSync(done.srtPath));
        assert.ok(fs.existsSync(done.vttPath));
      }
      return `All 4 queued items survived abrupt kill and completed after restart`;
    });

    // 9. Built-in Media Player HTTP Streaming & Offline Check
    await runTest("9. Offline mode & Media Player byte-range streaming", async () => {
      const mp3Path = path.join(FIXTURES_DIR, "format_en.mp3");
      const mp4Path = path.join(FIXTURES_DIR, "format_en.mp4");
      const mp3Res = await fetch(
        `${API_BASE}/media?path=${encodeURIComponent(mp3Path)}`,
        { headers: { Range: "bytes=0-1023" } }
      );
      assert.equal(mp3Res.status, 206);
      assert.equal(mp3Res.headers.get("content-type"), "audio/mpeg");
      assert.equal((await mp3Res.arrayBuffer()).byteLength, 1024);

      const mp4Res = await fetch(
        `${API_BASE}/media?path=${encodeURIComponent(mp4Path)}`,
        { headers: { Range: "bytes=0-511" } }
      );
      assert.equal(mp4Res.status, 206);
      assert.equal(mp4Res.headers.get("content-type"), "video/mp4");
      assert.equal((await mp4Res.arrayBuffer()).byteLength, 512);

      return `Offline active; 206 Partial Content verified for audio/mpeg and video/mp4`;
    });

    // 10. Clean-computer acceptance criterion (fresh app state dir + watch folder drop)
    await runTest("10. Clean-computer acceptance: fresh state + watch folder auto-export", async () => {
      const cleanStateDir = path.join(FIXTURES_DIR, "clean_computer_state");
      const cleanWatchDir = path.join(FIXTURES_DIR, "clean_watch_folder");
      fs.mkdirSync(cleanStateDir, { recursive: true });
      fs.mkdirSync(cleanWatchDir, { recursive: true });

      const cleanPort = TEST_PORT + 1;
      const cleanProc = launchApp(cleanStateDir, cleanPort);
      try {
        await waitForHealth(`http://127.0.0.1:${cleanPort}`);
        await apiCommand(
          "set_watch_folder",
          { folder: cleanWatchDir },
          `http://127.0.0.1:${cleanPort}`
        );

        const dropWav = path.join(cleanWatchDir, "meeting_notes.wav");
        generateSpeechWav(
          "Daniel",
          "Welcome to Audio Transcription Studio. Everything works automatically.",
          dropWav
        );

        const start = Date.now();
        while (Date.now() - start < 30000) {
          if (
            fs.existsSync(path.join(cleanWatchDir, "meeting_notes.txt")) &&
            fs.existsSync(path.join(cleanWatchDir, "meeting_notes.srt")) &&
            fs.existsSync(path.join(cleanWatchDir, "meeting_notes.vtt"))
          ) {
            break;
          }
          await sleep(250);
        }

        const txt = fs.readFileSync(path.join(cleanWatchDir, "meeting_notes.txt"), "utf8");
        const srt = fs.readFileSync(path.join(cleanWatchDir, "meeting_notes.srt"), "utf8");
        const vtt = fs.readFileSync(path.join(cleanWatchDir, "meeting_notes.vtt"), "utf8");
        assert.match(txt.toLowerCase(), /welcome|audio|automatically/);
        assert.match(srt, /00:00:00,\d{3} --> /);
        assert.ok(vtt.startsWith("WEBVTT\n\n"));

        return `Fresh instance exported TXT/SRT/VTT automatically: "${txt.trim()}"`;
      } finally {
        try {
          cleanProc.kill("SIGKILL");
        } catch {}
      }
    });
  } finally {
    cleanup();
  }

  console.log("\n=== E2E TEST SUMMARY ===");
  const passed = results.filter((r) => r.status === "PASS").length;
  const failed = results.filter((r) => r.status === "FAIL").length;
  for (const r of results) {
    console.log(`[${r.status}] ${r.name} (${r.ms}ms)`);
  }
  console.log(`Total: ${passed}/${results.length} passed, ${failed} failed.`);
  if (failed > 0) {
    process.exit(1);
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
