import {useState, type FormEvent} from 'react';
import {ArrowRight, ShieldCheck} from 'lucide-react';
import {createClient} from '../../api/client';
import {ErrorMessage} from '../../components/Alert';
import {useAsyncAction} from '../../hooks/useAsyncAction';
import './login.css';

type Props = {
  /** Shown when the session was locked because the token was rejected. */
  lockReason: string;
  onUnlock: (token: string) => void;
};

export function LoginPage({lockReason, onUnlock}: Props) {
  const [token, setToken] = useState('');
  const {busy, error, run} = useAsyncAction();

  function submit(event: FormEvent) {
    event.preventDefault();
    const candidate = token.trim();
    void run(async () => {
      await createClient(candidate).status();
      onUnlock(candidate);
    });
  }

  return (
    <div className="login">
      <div className="login-brand">
        <span className="brand-icon">p</span> passage<span className="preview">EARLY ACCESS</span>
      </div>
      <section className="login-card">
        <div className="eyebrow">YOUR SERVICES. ONE ENTRANCE.</div>
        <h1>
          A clear path
          <br />
          to your services.
        </h1>
        <p>Manage your reverse proxy from one quiet place.</p>
        <form onSubmit={submit}>
          <label htmlFor="token">Administrator token</label>
          <input
            id="token"
            type="password"
            autoComplete="off"
            required
            placeholder="Paste your local administrator token"
            value={token}
            onChange={(e) => setToken(e.target.value)}
          />
          <small>Read admin.token from your Passage data directory. Your token stays in this tab’s memory.</small>
          <button className="primary" disabled={busy}>
            {busy ? 'Connecting…' : 'Open your gateway'}
            <ArrowRight size={17} />
          </button>
        </form>
        <ErrorMessage>{error || lockReason}</ErrorMessage>
        <div className="login-foot">
          <ShieldCheck size={15} /> Local administration · Pingora engine
        </div>
      </section>
      <footer>Made for your infrastructure.</footer>
    </div>
  );
}
