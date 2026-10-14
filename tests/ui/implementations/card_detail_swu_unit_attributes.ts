/**
 * Guards sp-iszds1: card detail draws SWU attributes, not Pokémon's.
 */
import type { ReplayHarness } from '../replay';

export async function steps(h: ReplayHarness) {
  await h.wait_for_visible('dl', 6000);
  for (const label of ['Type', 'Cost', 'Aspects', 'Arena', 'Power', 'HP', 'Traits']) {
    await h.assert_text_present(label);
  }
  await h.assert_text_present('Unit');
  await h.assert_visible('a.facet[title="Find all Aggression cards"]');
  await h.assert_visible('a.facet[title="Find all Heroism cards"]');
  await h.assert_visible('a.facet[title="Find all Ground cards"]');
  await h.assert_element_count('img.energy', 0);
  await h.assert_element_count('a.facet[href*="pokedex"]', 0);
  for (const gone of ['Pokédex', 'Weakness', 'Resistance', 'Retreat', 'Evolves', 'Attacks']) {
    await h.assert_text_absent(gone);
  }
  await h.screenshot('final_state');
}
