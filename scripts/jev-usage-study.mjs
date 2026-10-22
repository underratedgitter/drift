#!/usr/bin/env node
// Asks Jev (TypeSafe System One) how realistic people would use Drift, and turns
// the typed answers into launch decisions in code.
//
// Method: each persona is judged against the current preview, a proposed launch
// build, and that build with exactly one feature taken away (an ablation). The
// drop in adoption probability is the feature's importance for that person.
// Design questions are Choices, asked twice with reversed option order because
// Choice answers can depend on option order; the two distributions are averaged.
//
// Jev's answers are calibrated common-sense judgments, not observed behaviour.
// They rank decisions; they do not replace testing with real people.
//
//   node scripts/jev-usage-study.mjs [output-directory]
//
// Needs TYPESAFE_API_KEY (environment, or an `export TYPESAFE_API_KEY=...` line
// in ~/.zshrc). The key is never printed or written.
/* global fetch, AbortSignal */
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { homedir } from 'node:os';
import path from 'node:path';
import process from 'node:process';
import console from 'node:console';
import { setTimeout as delay } from 'node:timers/promises';

const MODEL = 'jev-1.14.0';
const ENDPOINT = 'https://api.typesafe.ai/v1/systemone';
const CONCURRENCY = 4;
const output = path.resolve(process.argv[2] ?? `research/jev-usage-${new Date().toISOString().slice(0, 10)}`);

async function apiKey() {
  if (process.env.TYPESAFE_API_KEY) return process.env.TYPESAFE_API_KEY;
  try {
    const profile = await readFile(path.join(homedir(), '.zshrc'), 'utf8');
    const match = profile.match(/^\s*(?:export\s+)?TYPESAFE_API_KEY\s*=\s*['"]?([^'"\s#]+)['"]?/m);
    if (match && !match[1].startsWith('$')) return match[1];
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
  }
  throw new Error('Set TYPESAFE_API_KEY to run the Jev usage study.');
}

const personas = {
  priya: {
    who: 'Priya, 34, marketing coordinator, not technical. Uses an iPhone 15 and a Windows 11 laptop at home.',
    task: 'Move 200 to 500 photos and short videos from her iPhone to her Windows laptop about once a month, to edit and back them up.',
    network: 'Home Wi-Fi.',
    current_methods: 'Sends batches to herself on WhatsApp (which compresses them), or plugs in a USB cable and fights with the Windows Photos import.',
  },
  marcus: {
    who: 'Marcus, 27, sales rep, comfortable with apps but not a programmer. Uses a Samsung Galaxy phone and a MacBook Air.',
    task: 'Send screenshots, PDFs and phone videos between his phone and laptop several times a day.',
    network: 'Home and office Wi-Fi.',
    current_methods: 'Telegram saved messages and Google Drive, because AirDrop does not work with Android.',
  },
  elena: {
    who: 'Elena, 41, freelance video editor. Uses a Mac Studio, a Windows workstation for rendering, and an iPhone.',
    task: 'Move 20 to 200 GB project folders with thousands of files between the Mac and the Windows PC every week.',
    network: 'Studio Wi-Fi and Ethernet on the same router.',
    current_methods: 'Copies to an external SSD and walks it over, or uploads to WeTransfer overnight.',
  },
  sam: {
    who: 'Sam, 30, software developer who avoids cloud services for privacy. Uses an Ubuntu Linux laptop and an Android phone.',
    task: 'Send documents, APKs and photos between phone and laptop a few times a week without any cloud account.',
    network: 'Home Wi-Fi.',
    current_methods: 'LocalSend, KDE Connect, or scp from a terminal.',
  },
  grace: {
    who: 'Grace, 45, office manager at a 10-person accounting firm. Staff use Windows PCs and company iPhones. An outside IT contractor manages the PCs.',
    task: 'Get scanned contracts and phone photos of receipts from staff phones onto office PCs every day.',
    network: 'Office Wi-Fi; visitors use a guest network.',
    current_methods: 'Staff email attachments to themselves, which bounce when files are large.',
  },
  kenji: {
    who: 'Kenji, 20, university student. Uses an Android phone, a Windows laptop and an iPad.',
    task: 'Share lecture recordings, slides and notes between his own devices and with classmates sitting next to him.',
    network: 'Campus Wi-Fi, which usually blocks devices from seeing each other, and his phone hotspot.',
    current_methods: 'Google Drive and Discord.',
  },
  linda: {
    who: 'Linda, 66, retired teacher with low confidence in technology. Uses an iPad, an iPhone and an old Windows desktop.',
    task: 'Copy videos of her grandchildren from her iPhone to the Windows desktop to keep them safe.',
    network: 'Home Wi-Fi set up by her son.',
    current_methods: 'Emailing videos to herself, which fails for anything long, then asking her son for help.',
  },
  ravi: {
    who: 'Ravi, 38, travel photographer. Uses an Android phone and a MacBook Pro.',
    task: 'Move 10 to 30 GB of RAW photos and drone videos from his phone to his laptop while travelling.',
    network: 'Hotel Wi-Fi that isolates guests, or no internet at all, so he uses his phone hotspot.',
    current_methods: 'USB-C cable with Android File Transfer, which often disconnects mid-copy.',
  },
};

// Every launch-plan fact, and the current-preview fact it replaces in an ablation.
const features = {
  platforms: {
    label: 'Native apps on all five platforms',
    launch: 'Native apps for iPhone/iPad, Android, macOS, Windows and Linux from the usual app stores and website. Installers are about 10 MB.',
    current: 'Desktop apps for macOS and Windows (135 MB installers) and an Android app. iPhone and iPad only work through a web page that first requires installing a security certificate profile in iOS Settings.',
  },
  signed: {
    label: 'Signed, notarized installers',
    launch: 'Installers are signed and notarized, so macOS and Windows open them without warnings.',
    current: 'Installers are unsigned: macOS says it cannot check the app for malicious software and Windows shows "Windows protected your PC"; the user has to find the hidden way to open it anyway.',
  },
  pairing: {
    label: 'Tap a nearby device and compare a 6-digit code',
    launch: 'To connect two devices the first time, tap the other device in the Nearby list, then check that both screens show the same 6-digit number and press Accept on the other device.',
    current: 'To connect two devices, open "Pair a device" on the receiver, type its 30-character code on the sender (or paste a long invitation link), approve on the receiver, then press "Confirm connection" on the sender.',
  },
  remembered: {
    label: 'Devices stay paired after restarts',
    launch: 'Once connected, the two devices stay paired, including after closing the app or restarting; next time they just appear as ready.',
    current: 'A pairing is forgotten when either app closes and after 24 hours; the user pairs again each time.',
  },
  two_way: {
    label: 'One pairing works in both directions',
    launch: 'One pairing lets either device send to the other.',
    current: 'A pairing only works in one direction; to send back, the user pairs again from the other device.',
  },
  auto_accept: {
    label: 'Optional auto-accept from your own devices',
    launch: 'Files from your own paired devices can arrive automatically (a setting, on by default for devices you mark as yours); from anyone else you approve the file list first.',
    current: 'The receiver must review and approve every incoming file list, even from their own devices.',
  },
  save_location: {
    label: 'Files land in Downloads / Files / Photos',
    launch: 'Received files go straight to the Downloads folder on computers, the Files app on iPhone and the Downloads folder on Android, with a "Show in folder" button; photos and videos can be saved to the photo library in one tap.',
    current: 'Received files are kept in a hidden app data folder with a long random name; on phones each file has to be exported one at a time through a save dialog.',
  },
  share_sheet: {
    label: 'Send from the share sheet',
    launch: 'On phones, Drift appears in the Share menu of Photos, Files and other apps, and on computers files can be dropped on the window or tray icon.',
    current: 'Files can only be chosen from inside the Drift app with its own file picker.',
  },
  background: {
    label: 'Receives in the background on computers',
    launch: 'On computers Drift keeps running in the menu bar or system tray, so a computer can receive while its window is closed.',
    current: 'Drift only receives while its window is open.',
  },
  qr_scan: {
    label: 'Camera QR scanning',
    launch: 'If a device does not appear in the Nearby list, scan a QR code on the other screen with the phone camera.',
    current: 'If a device does not appear in the Nearby list, the user types an IP address, a port number and the 30-character code.',
  },
  updates: {
    label: 'Automatic updates',
    launch: 'The app updates itself securely in the background.',
    current: 'Each new version must be downloaded and installed by hand.',
  },
};
const common = {
  network: 'Works between devices on the same Wi-Fi network or phone hotspot. It does not send over the internet.',
  security: 'End-to-end encrypted, files are checked for corruption on arrival, and interrupted transfers resume.',
  speed: 'Usually several times faster than cloud upload and download because files go directly over the local network.',
  price: 'Free, with no account or sign-up.',
};
const internetFeature = 'If the devices are not on the same network, Drift sends through an encrypted relay over the internet instead, more slowly.';

function product(kind, removed) {
  const facts = Object.fromEntries(Object.entries(features).map(([key, feature]) => [key, kind === 'current' || key === removed ? feature.current : feature.launch]));
  return { ...facts, ...common, ...(kind === 'launch_plus_internet' ? { internet: internetFeature } : {}) };
}
const variants = {
  current_preview: product('current'),
  launch: product('launch'),
  launch_plus_internet: product('launch_plus_internet'),
  ...Object.fromEntries(Object.keys(features).map(key => [`launch_without_${key}`, product('launch', key)])),
};

const adoptionQuestions = {
  completes_first_transfer: {
    type: 'noul',
    instructions: 'Read `persona` and `product`. Using only the product exactly as described, would this person get through installing it on both devices, connecting them, and finishing their first transfer for `persona.task` on `persona.network`, without giving up and going back to `persona.current_methods`?',
  },
  keeps_using: {
    type: 'noul',
    instructions: 'Read `persona` and `product`. Suppose this person did finish one transfer with the product exactly as described. One month later, would they still be choosing it for `persona.task`, instead of `persona.current_methods`?',
  },
};

// Options are listed in one order; the study also asks them reversed. Design
// questions see only the persona: with the preview's behaviour in the state, Jev
// read "expect" literally and picked whatever the preview already does.
const designQuestions = {
  pairing_method: {
    instructions: 'Which way of connecting two devices for the first time would this person be most likely to complete correctly on the first try, without help?',
    criteria: {
      tap_and_compare: 'Tap the other device in a Nearby list, check both screens show the same 6-digit number, and press Accept.',
      scan_qr: 'Point the phone camera at a QR code shown on the other screen.',
      type_short_code: 'Type a 6-digit code shown on the other screen.',
      paste_link: 'Copy a long invitation link on one device and paste it on the other.',
      type_long_code: 'Type a 30-character code plus an IP address and port.',
    },
  },
  receive_policy: {
    instructions: 'Which rule for incoming files would this person want for files sent from their own devices?',
    criteria: {
      auto_accept_own: 'Files from devices they have marked as their own arrive automatically; files from anyone else need approval.',
      ask_every_time: 'Show the file list and ask them to approve every transfer, even from their own devices.',
      accept_anyone: 'Accept files from any device on the network automatically.',
    },
  },
  save_location: {
    instructions: 'Where would this person want files they receive from their own devices to be saved, so they can find and use them afterwards?',
    criteria: {
      downloads: 'The normal Downloads folder (Files app on iPhone), with a button to show the file.',
      photo_library: 'Photos and videos in the phone or computer photo library; other files in Downloads.',
      ask_each_time: 'A save dialog asking where to put each transfer.',
      app_folder: 'A folder inside the app that they open from the app.',
    },
  },
};
const diagnosisQuestions = {
  deal_breaker: {
    instructions: 'If this person stopped using the current preview described in `current_preview`, which single problem would most likely be the reason?',
    criteria: {
      install_warnings: 'Security warnings when installing on macOS or Windows.',
      no_iphone_app: 'No real iPhone/iPad app; the certificate-profile web page.',
      pairing_effort: 'Typing long codes and the approve-then-confirm steps.',
      repairing: 'Having to pair again after closing the app, and pairing separately for each direction.',
      hidden_files: 'Received files hidden in an app folder or exported one at a time.',
      network_blocked: 'Devices cannot see each other on their Wi-Fi.',
      no_internet_mode: 'Cannot send when the devices are on different networks.',
    },
  },
};
const contextQuestions = {
  needs_internet_often: { type: 'noul', instructions: 'For `persona.task`, would this person often need to send between devices that are not on the same Wi-Fi network or hotspot?' },
  network_likely_blocks_lan: { type: 'noul', instructions: 'Is `persona.network` likely to stop two devices on it from discovering or connecting to each other directly?' },
  expects_background_on_phone: { type: 'noul', instructions: 'Would this person expect a long transfer to keep going while their phone is locked or they switch to another app?' },
  cares_about_install_size: { type: 'noul', instructions: 'Would a 135 MB installer instead of a 10 MB one noticeably reduce this person\'s willingness to install the app?' },
};

const reversed = criteria => Object.fromEntries(Object.entries(criteria).reverse());
const choices = (definitions, order) => Object.fromEntries(Object.entries(definitions).map(([id, question]) => [id, { type: 'choice', instructions: question.instructions, criteria: order === 'reversed' ? reversed(question.criteria) : question.criteria }]));
const designRequest = (persona, order) => ({ model: MODEL, state: { persona }, questions: { ...contextQuestions, ...choices(designQuestions, order) } });
const diagnosisRequest = (persona, order) => ({ model: MODEL, state: { persona, current_preview: variants.current_preview }, questions: choices(diagnosisQuestions, order) });

async function ask(key, body, attempt = 0) {
  const response = await fetch(ENDPOINT, { method: 'POST', headers: { Authorization: `Bearer ${key}`, 'Content-Type': 'application/json' }, body: JSON.stringify(body), signal: AbortSignal.timeout(90_000) });
  if ((response.status === 429 || response.status === 529 || response.status >= 500) && attempt < 5) {
    const wait = Number(response.headers.get('retry-after')) * 1000 || 500 * 2 ** attempt;
    await delay(wait);
    return ask(key, body, attempt + 1);
  }
  if (!response.ok) throw new Error(`Jev request failed with HTTP ${response.status}: ${(await response.text()).slice(0, 500)}`);
  return response.json();
}

async function pool(jobs, worker) {
  const results = new Array(jobs.length);
  let next = 0;
  await Promise.all(Array.from({ length: Math.min(CONCURRENCY, jobs.length) }, async () => {
    while (next < jobs.length) { const index = next++; results[index] = await worker(jobs[index]); }
  }));
  return results;
}

const mean = values => values.reduce((sum, value) => sum + value, 0) / values.length;
const round = value => Math.round(value * 1000) / 1000;

async function main() {
  const key = await apiKey();
  await mkdir(output, { recursive: true });
  const jobs = [];
  for (const [personaId, persona] of Object.entries(personas)) {
    for (const [variantId, variant] of Object.entries(variants)) jobs.push({ kind: 'adoption', personaId, variantId, body: { model: MODEL, state: { persona, product: variant }, questions: adoptionQuestions } });
    for (const order of ['listed', 'reversed']) {
      jobs.push({ kind: 'design', personaId, order, body: designRequest(persona, order) });
      jobs.push({ kind: 'diagnosis', personaId, order, body: diagnosisRequest(persona, order) });
    }
  }
  console.log(`Asking ${MODEL}: ${jobs.length} requests…`);
  let usage = 0;
  const answered = await pool(jobs, async job => {
    const reply = await ask(key, job.body);
    usage += reply.usage?.input_tokens ?? 0;
    return { ...job, model: reply.model, answers: reply.answers };
  });

  // Adoption = P(finishes first transfer) × P(still using a month later | finished).
  const adoption = {};
  for (const item of answered.filter(item => item.kind === 'adoption')) {
    const completes = item.answers.completes_first_transfer.noul, keeps = item.answers.keeps_using.noul;
    (adoption[item.variantId] ??= {})[item.personaId] = { completes: round(completes), keeps: round(keeps), adoption: round(completes * keeps) };
  }
  const personaIds = Object.keys(personas);
  const variantSummary = Object.fromEntries(Object.entries(adoption).map(([variant, byPersona]) => [variant, {
    completes: round(mean(personaIds.map(id => byPersona[id].completes))),
    keeps: round(mean(personaIds.map(id => byPersona[id].keeps))),
    adoption: round(mean(personaIds.map(id => byPersona[id].adoption))),
  }]));

  // Importance = how much adoption falls when only this feature reverts to the preview.
  const MUST = 0.10, SHOULD = 0.03, DEAL_BREAKER = 0.25;
  const importance = Object.entries(features).map(([id, feature]) => {
    const drops = Object.fromEntries(personaIds.map(persona => [persona, round(adoption.launch[persona].adoption - adoption[`launch_without_${id}`][persona].adoption)]));
    const meanDrop = round(mean(Object.values(drops)));
    const dealBreakerFor = Object.entries(drops).filter(([, drop]) => drop >= DEAL_BREAKER).map(([persona]) => persona);
    const tier = meanDrop >= MUST || dealBreakerFor.length ? 'must-have for launch' : meanDrop >= SHOULD ? 'should-have' : 'defer';
    return { id, label: feature.label, meanDrop, tier, dealBreakerFor, drops };
  }).sort((a, b) => b.meanDrop - a.meanDrop);
  const internetGain = round(variantSummary.launch_plus_internet.adoption - variantSummary.launch.adoption);

  // Design choices: average each persona's two option orders, then average personas.
  const design = {};
  const context = {};
  const kinds = { ...Object.fromEntries(Object.keys(designQuestions).map(id => [id, 'design'])), ...Object.fromEntries(Object.keys(diagnosisQuestions).map(id => [id, 'diagnosis'])) };
  const definitions = { ...designQuestions, ...diagnosisQuestions };
  for (const [id, kind] of Object.entries(kinds)) {
    const perPersona = {};
    for (const persona of personaIds) {
      const replies = answered.filter(item => item.kind === kind && item.personaId === persona).map(item => item.answers[id]);
      const options = Object.keys(definitions[id].criteria);
      const probabilities = Object.fromEntries(options.map(option => [option, round(mean(replies.map(reply => reply.probabilities[option] ?? 0)))]));
      const orderAgreement = replies[0].choice === replies[1].choice;
      perPersona[persona] = { choice: Object.entries(probabilities).sort((a, b) => b[1] - a[1])[0][0], probabilities, orderAgreement };
    }
    const options = Object.keys(definitions[id].criteria);
    const overall = Object.fromEntries(options.map(option => [option, round(mean(personaIds.map(persona => perPersona[persona].probabilities[option])))]));
    design[id] = { overall, winner: Object.entries(overall).sort((a, b) => b[1] - a[1])[0][0], perPersona };
  }
  for (const id of Object.keys(contextQuestions)) {
    const perPersona = Object.fromEntries(personaIds.map(persona => [persona, round(mean(answered.filter(item => item.kind === 'design' && item.personaId === persona).map(item => item.answers[id].noul)))]));
    context[id] = { mean: round(mean(Object.values(perPersona))), perPersona };
  }

  const models = [...new Set(answered.map(item => item.model))];
  const summary = { model: models, askedAt: new Date().toISOString(), requests: jobs.length, inputTokens: usage, thresholds: { MUST, SHOULD, DEAL_BREAKER }, variantSummary, importance, internetGain, design, context, adoption };
  await writeFile(path.join(output, 'requests.json'), JSON.stringify({ personas, features, common, internetFeature, adoptionQuestions, designQuestions, diagnosisQuestions, contextQuestions }, null, 2) + '\n');
  await writeFile(path.join(output, 'answers.json'), JSON.stringify(answered.map(item => { const copy = { ...item }; delete copy.body; return copy; }), null, 2) + '\n');
  await writeFile(path.join(output, 'summary.json'), JSON.stringify(summary, null, 2) + '\n');
  console.log(JSON.stringify({ variantSummary, importance: importance.map(item => { const copy = { ...item }; delete copy.drops; return copy; }), internetGain, design: Object.fromEntries(Object.entries(design).map(([id, value]) => [id, { winner: value.winner, overall: value.overall }])), context: Object.fromEntries(Object.entries(context).map(([id, value]) => [id, value.mean])), inputTokens: usage, models }, null, 2));
}

main().catch(error => { console.error(error.message); process.exitCode = 1; });
