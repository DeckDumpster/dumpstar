/**
 * Hand-written implementation for homepage_tagline_names_swu.
 *
 * Guards the copy sweep (sp-tyh0wx): the home tagline names Star Wars:
 * Unlimited, and the Pokémon tagline the site was copied with is gone.
 */
import type { ReplayHarness } from '../replay';

export async function steps(h: ReplayHarness) {
  await h.navigate('/');
  await h.wait_for_visible('.tagline');
  await h.assert_text_present('A Star Wars: Unlimited collection tracker.');
  await h.assert_text_absent('Pokémon');
  await h.screenshot('home');
}
