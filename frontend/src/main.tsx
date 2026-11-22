import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import './index.css';
import App from './App';

const syncHidden = () => document.documentElement.classList.toggle('app-hidden', document.hidden);
document.addEventListener('visibilitychange', syncHidden);
syncHidden();

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>
);
