import './fonts';
import {AbsoluteFill, Composition, Sequence} from 'remotion';
import {T} from './theme';
import {Scene1} from './scenes/Scene1_ColdOpen';
import {Scene2} from './scenes/Scene2_Enter';
import {Scene3} from './scenes/Scene3_Krc';
import {Scene4} from './scenes/Scene4_Verify';
import {Scene5} from './scenes/Scene5_OnChain';
import {Scene6} from './scenes/Scene6_EndCard';

const Launch: React.FC = () => {
  return (
    <AbsoluteFill style={{backgroundColor: T.bg}}>
      <Sequence from={0} durationInFrames={180}>
        <Scene1 />
      </Sequence>
      <Sequence from={180} durationInFrames={240}>
        <Scene2 />
      </Sequence>
      <Sequence from={420} durationInFrames={240}>
        <Scene3 />
      </Sequence>
      <Sequence from={660} durationInFrames={240}>
        <Scene4 />
      </Sequence>
      <Sequence from={900} durationInFrames={240}>
        <Scene5 />
      </Sequence>
      <Sequence from={1140} durationInFrames={180}>
        <Scene6 />
      </Sequence>
    </AbsoluteFill>
  );
};

export const Root: React.FC = () => {
  return (
    <Composition
      id="Launch"
      component={Launch}
      durationInFrames={1320}
      fps={60}
      width={1920}
      height={1080}
    />
  );
};
