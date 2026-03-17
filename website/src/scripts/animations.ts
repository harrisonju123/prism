import { gsap } from 'gsap';
import { ScrollTrigger } from 'gsap/ScrollTrigger';

gsap.registerPlugin(ScrollTrigger);

// Nav: transparent → frosted on scroll
const nav = document.getElementById('nav');
if (nav) {
  window.addEventListener('scroll', () => {
    if (window.scrollY > 40) {
      nav.style.backgroundColor = 'rgba(10, 10, 15, 0.85)';
      nav.style.backdropFilter = 'blur(12px)';
      nav.style.borderBottom = '1px solid #1e1e2e';
    } else {
      nav.style.backgroundColor = 'transparent';
      nav.style.backdropFilter = 'none';
      nav.style.borderBottom = '1px solid transparent';
    }
  }, { passive: true });
}

// Hero: staggered entrance on page load
const heroTl = gsap.timeline({ defaults: { ease: 'power2.out' } });

heroTl
  .to('#hero-h1', { opacity: 1, y: 0, duration: 0.8, clearProps: 'transform' }, 0.1)
  .to('#hero-sub', { opacity: 1, duration: 0.6, clearProps: 'transform' }, 0.4)
  .to('#hero-ctas', { opacity: 1, duration: 0.6, clearProps: 'transform' }, 0.6)
  .to('#hero-version', { opacity: 1, duration: 0.5, clearProps: 'transform' }, 0.8);

// Feature cards: staggered scroll reveal
const featureCards = document.querySelectorAll<HTMLElement>('.feature-card');
if (featureCards.length > 0) {
  gsap.to(featureCards, {
    opacity: 1,
    y: 0,
    duration: 0.5,
    stagger: 0.1,
    ease: 'power2.out',
    clearProps: 'transform',
    scrollTrigger: {
      trigger: '#features-grid',
      start: 'top 80%',
      once: true,
    },
  });
}

// Download section: fade + slide up on scroll
ScrollTrigger.create({
  trigger: '#download-inner',
  start: 'top 85%',
  once: true,
  onEnter: () => {
    gsap.to('#download-inner', {
      opacity: 1,
      y: 0,
      duration: 0.6,
      ease: 'power2.out',
      clearProps: 'transform',
    });
  },
});
