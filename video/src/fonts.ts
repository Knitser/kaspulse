/* Load the real brand fonts so the render matches the site: JetBrains Mono
   (data / signatures / the whole verifiable identity) + Inter (display/body).
   Imported once from Root. */
import {loadFont as loadJetBrains} from '@remotion/google-fonts/JetBrainsMono';
import {loadFont as loadInter} from '@remotion/google-fonts/Inter';

loadJetBrains('normal', {weights: ['400', '500', '600', '700', '800'], subsets: ['latin']});
loadInter('normal', {weights: ['400', '500', '600', '700', '800'], subsets: ['latin']});
