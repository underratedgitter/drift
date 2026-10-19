import { useEffect, useState } from 'react';
import QRCode from 'qrcode';
type Info = { enabled?: boolean; url: string; origin: string; expires: number; used: boolean; fingerprint: string; certificate: string; profile: string };
export function PhonePanel({ request, report }: { request: <T>(path: string, body?: unknown) => Promise<T>; report: (message: string) => void }) {
  const [info, setInfo] = useState<Info | null>(null);
  const [qr, setQr] = useState('');
  useEffect(() => { void request<Info>('/phone').then(setInfo).catch((error: Error) => report(error.message)); }, []);
  useEffect(() => { if (info?.url) void QRCode.toDataURL(info.url, { width: 220, margin: 2 }).then(setQr).catch(() => report('Could not generate phone QR. Copy the invitation instead.')); }, [info?.url]);
  if (!info) return <section className="panel phone-panel"><h2>Phone connection</h2><p>Loading secure phone access…</p></section>;
  if (info.enabled === false) return <section className="panel phone-panel"><h2>Phone access is disabled</h2><p>Restart Drift without --no-phone to enable the dedicated HTTPS phone gateway.</p></section>;
  const download = (content: string, name: string, type: string) => {
    const url = URL.createObjectURL(new Blob([content], { type }));
    const link = document.createElement('a'); link.href = url; link.download = name; link.click();
    setTimeout(() => URL.revokeObjectURL(url), 2000);
  };
  return <section className="panel phone-panel"><div className="section-heading"><h2>Your phone, in the picture.</h2><span className="subtle">Android & iPhone browser app</span></div>
    <p>Phone access has its own HTTPS address and limited permissions. Never share the Mac’s private control URL.</p>
    <details className="phone-trust" open><summary>First time? Set up certificate trust.</summary>
      <p>Export the public certificate below and transfer it to your phone using a channel you already trust, such as a cable or AirDrop. Do not download replacement certificates over untrusted HTTP or bypass browser certificate warnings.</p>
      <div className="drop-actions"><button className="button secondary" onClick={() => download(info.profile, 'Drift-Phone.mobileconfig', 'application/x-apple-aspen-config')}>Export iPhone profile</button><button className="button secondary" onClick={() => download(info.certificate, 'Drift-Phone-CA.crt', 'application/x-x509-ca-cert')}>Export Android certificate</button></div>
      <p><strong>iPhone / iPad:</strong> open the profile, install it in Settings → General → VPN & Device Management, then enable Drift under Settings → General → About → Certificate Trust Settings.</p>
      <p><strong>Android:</strong> install the CA certificate in Settings → Security / Privacy → Encryption & credentials → Install a CA certificate. Labels vary by manufacturer.</p>
      <p>Verify the certificate’s SHA-256 fingerprint before trusting it:</p><code className="certificate-fingerprint">{info.fingerprint.match(/.{1,2}/g)?.join(':')}</code>
      <p>The certificate-issuing private key is not retained. Remove the profile/CA when you stop using Drift. A changed Mac IP address or expired certificate needs a new setup.</p>
    </details>
    <div className="phone-invite">{qr && <img src={qr} width="180" height="180" alt="Temporary phone pairing invitation"/>}<div><h3>Scan with your phone’s camera</h3><p>{info.origin}</p><p>{info.used ? 'Used. Generate a new invitation for another phone.' : `One use. Expires ${new Date(info.expires * 1000).toLocaleTimeString()}.`}</p><button className="button" onClick={() => void request<Info>('/phone/invitation', {}).then(setInfo).catch((error: Error) => report(error.message))}>New phone invitation</button><button className="text-button" onClick={() => void navigator.clipboard.writeText(info.url).catch(() => report('Clipboard is unavailable. Scan the QR instead.'))}>Copy phone invitation</button></div></div>
    <p>Approve the incoming phone on this Mac, then confirm on the phone. Keep both apps open during transfers. On iPhone use Safari → Share → Add to Home Screen; on Android use Chrome → Install / Add to Home screen.</p>
  </section>;
}
