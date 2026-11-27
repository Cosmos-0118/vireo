import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import './index.css';
import App from './App';

// Platform-specific CSS (index.css): macOS renders text larger than Windows at the same px.
if (/Macintosh|Mac OS X/i.test(navigator.userAgent)) document.documentElement.classList.add('platform-mac');

const syncHidden = () => document.documentElement.classList.toggle('app-hidden', document.hidden);
document.addEventListener('visibilitychange', syncHidden);
syncHidden();

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>
);
