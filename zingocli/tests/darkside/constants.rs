use crate::darkside::tests::TreeState;

pub const GENESIS_BLOCK: &'static str = "0400000027e30134d620e9fe61f719938320bab63e7e72c91b5e23025676f90ed8119f02c94c1ef3fa94b348f1b6d1bdf5899ad1eac7bfbf0de6d2cd1a4bedf84699b09c415071fecf148b458b159d1d310b2d072e8250bfefcd1a7ead2b309071f6e954ecf581640f0f0f201b0092b34257e0a52315e66f950db025ae0b0b3b6c71aa017c8a2348b4dd00002400f5c2c1b120d8abc30adf542ee48f6301eb0f730839c15bfcdbb42df0a8d4a326cd9389010400008085202f89010000000000000000000000000000000000000000000000000000000000000000ffffffff03510101ffffffff000000000001000000c041bfdaffffffff0001fc0ffba31852cdff6ccbbb61157848f9fb42e6a6eb831806a5e1955f21dc943c5fcb897f9314fc5ebac4510b86706f45458c6db1d36dbab6ee23315e9654c54cfdd8aaf1af233dee596a2f03d42315fde064eade886a2b10843807d9498e9a83604227bb3dcc516b05c1f87ff343099d89d8fe27196be88ea0f6a59503bbdf4992a740469cd449dd253e18eaeb6392e435e22d3d1dc75530b30a06f1ae3ea8dab48e637cea7747a571f7539e338461fd5938581878f9b6111b59de5c25d93043b99c8472d04bb0300eaa68eeba6ccac5de0d36fe25bf3639d58fbc2480b8fcab31dada9929d9bf44515c5d1517817047d22badd35a37f174b3803fe6a9dece7f11c93af94c33eb8b442bc94d728452a545ce35a41ad2580c8c737a245d5cdc5f36df5bdd4ed27ffb6568893c909c690c95d6db22f09680f4febe906d456070dc9363b497c67b854125572a811a24a65b63e191e09026b7409b71e18e5fe62d3d1840a557d4b222e07e00dac01131f0993a91914e70ed8f6a609f0c894f6eeaf3b35fcc830159851fff0d38ed3c0f15acb3d4de404ead0bfdd9c10d169cdd4bb1cf4a074349c330b57636b093a9b3b749ed14ef347326b313267f34be94ec9b9b7ae41d4ccd7364ed647a5a74b432cd3c063d5b2bbaedddba11e25107ec4e89c41ff45173ae49e3a323280aa9a3cdc8d9a7c264d1f08cc2bcc486d02fa2158054aaa55e82448e827a7a50778d04308a99401eeb7c835568c15ca2104bd05ebe569eed9ac64ef63143eeb4bf203ac6215cd63f921bfc2b3d704533cb32a82fbbe4caa677ee1a970e10dd4a36b4a8cd3003adc2962776a74aadb5577c457664a4970fa0c78d4edcb48e5743433d5f4d329148bf2d4a6c3f536ebad618ece61ebe09c60a8ec0ac73d9c6ba0429ac4a63d6b37db740fff665c9a2daf2b8c24f0269a9fe34ea99578926efa81b40da08d66bc4c04ccbf0f6a96a0bbda3966282af280600a686365ab3eb47a46a4204d40340aea39c0ebb910a786e0b88a115d9d9d95e1661d4548ba1cb5941082d3249aa37444b844fb1932e81d4427efad6ebc4addff3daf3246a0f77273aa1ad352fcbddf4fa617bcee201eae7c763737f2168d236fa764e17addf1ecb42706e30918378b1b2a0aca3233a129f55685264c6f5a49867fdaa73797616c785d18a707805c2514e91af9015f0cbf2097be74ee4ee81eed4723afd0d45f656c539e2e6e4578ad342d84a2d812199486e0d57094125551fd3a268a383c1f9642809858b23aa11a7e242012f49c0d66417177d4cd0de2ca4430929a7e50e9cd8b569cfd17cf648473e80cea7003e4aa0cc382d6bf962bed6a00cade090c0b43ff689dcd535085e2aac6fe807e6263b2564b86c469baeb3586c1e0db5039ced9ac723f3911e28e396655e825101";
pub const DARKSIDE_SEED: &'static str =
"still champion voice habit trend flight survey between bitter process artefact blind carbon truly provide dizzy crush flush breeze blouse charge solid fish spread";
pub const TRANSACTION_INCOMING_100TAZ: &'static str = "050000800a27a726b4d0d6c2000000001600000000000143bef95ecd8db46ff669821b13c5bc54a8341ea19bc8b27a0a31188c458a4a0ed3cbe18dc9e763f9e6ed7309c733428ee37a38d5de4cb217806e621fa2bd751128f2f52868687bf0f672b24bf230851fd7e6bc407e9417d435ba1578be813cea02997f254fc0a8fa75ddbb84f4a4a19ad3cce79fb31c6b9fc93bf72f01e1be2e69af9a5c64b7e34160cb0be709fe7006ee3233053b266254cb6f8b1c457f6491108ccc882222f5aa2f05e1914fdf771265691361ed43c7b2b85f93bc53c49960f061fd21ee13b1d76e04ed9ff172e8d94ffd4e30a5f4df2b5ac0473d2ff3439568e949108a563da66a79d00b476747d57834d65d75daf91692a940c8b8d1a182e4740f3297934238df2093911b977e36388c54921076b748d4427b3dcf4e91b35e22acd6fdb3fad47dc01340ba33a6bfa3710ecccddcbbe177678eedfb6263b7fab0c8ec754a233cf25c3c8330c87b9eab1759cc913f307999ba204d3588cdb327ad11e3263356f9b2424e0c248976116109c7ad41144277e739dc0ba02c790661e0cd8c2e59c9ee4b40d52861c9728057d7fc9fb92b02fa6498465b26e6605a75d233a665f15e4a9e73ab5b2b9c081567b9f99f7706d0faf468aff5bcffc5e2c6a28d3a55e95ad19d240aab94527e372b675ce451e9122a6e9ec089310a7215a87dd9c374c79769e5aef9b5cb5a6a1c8e9a14584daf9465d5a92098916f1091246fdff5df936d2b13f65f9b7cccf64e39cffce1121465c0a04a180c0b916acbd76ef968b785238779d0cb8d22a90b5018d1b45978b41ef59aef32843b183e4a8d81ea6a508ca94f3c2f56f0a8e5f654bbb7941e63ee82953ccc4a80bd1179efe5bac54d0673c3dcc6b1f49aa38ef0b614c29387eca8b5423184c21a2d4527e52085ef70c688f64eb9a8b87233b993de680adee3d8cb7ebe0aa3bde4df0a0dd56ab373dc08c9ed9da8b43cdbdc9ef3df1c12026f718e208d39801e716bf0d614bdc38148f3b551248dc07f8a550ad1255fe574f4bc6344e5f5e63ce8184aa07d3ab39ef813b95b073fc64ce43aa77633483654b9f3cfa83f0b828c1841c9e8a723ca615097843633543c61e7850cc3e2a0f1f3f0d5b5e0280bd5efb76f1de4cf9ab9386a04ab79b92fa46379b2c22a47fbcb32e64c65dc2e439a2317474cd45cec08d9704a24980a4e9b2cbc890c6976fce14a3c1bbe0f2c7182e2680c8fa952001da32c5a89056178606c78dc854625805eeade5282634da21b00e4478759016a3f4a84436f6b955872f28876bfd4b03771867d2f556a80b60b4ce72ef266ec3e029315b39695385f7ffdfb64a0af14d53812263691c0d0ed3b13b53362a2db578814cba37017f94957c492e22fb80814d9d1da7c82357b213e4de317cc4f4231860238dbf22d7de0a96cdba9ec65153153a96634b16d1abd7ae4f65abe46a5bb4696b4f5ff191d333d2c0c0ddf35d5337eaaa722dcbc450c2b0c745f5b35a6c9034c5f0a5c9bb857a874a303d3b50fcbccc6dfc2b37a61daeddb4a00e6e55da17f67afcdaf737e6530413728be8ac4008eb33fbdeb37bcf253c72c1eba6cd9f2dcbff5dbeea5dbb8885ac4e55874bcf09a40afc48e946a7656bd3eb2552a6c30c41bbd708d1f12f4c328b92777092dd1640920177a89b75635a4f0dd6691c609e6aca055c66e39dce347389cf199c05a799016ccdea7834a6b8d1444186466947f2d235da802c730cc3c2e66082f605cb109c3ff4cbb2933030e890f999ed8ceac5c7dcc72eab5bb7302c5507810b376ed5c6363ac3fe0aab397caa41bd40e7ccdb0692661bde267720104661b3345a9363a9f97681ee538c7169b973d3bbf479c386f0b30bd0eddc60a09aba026c92c7f6bab6ecfd97645d481a647a0958a46e6da2bece03715f23e6f4e397a771c4916a47b3e250e0c4867779643f5fbe2ddf1c2cfb9b2b3ad8396e2ad3123c06d51d2dd0a1c671f42e93f06d48de265ada81dbbc210d08fb461b43f969ab4fcbebf5a2c340114a807cfcca026f058a7f7625b9b3e1e633e27422eaa8ead2811d1ed30edd111ff158fb4b18e282b50dd2c0b722c78e377fb269117293adb395b6d8d743e3bd6aa8d40c6d8ab80b8ea3e4cfffe53d73abc0c8026a4b60294392518496f345e5363bca2f18356145211e7cf89b92b3bdafd04abbc11e69cafe70aeec622aa14ca988eb5da764f1ec1ac97f5f5d2f3d26c2eefabb3ee6edda1a17b83b27b51f2fe566fba0df04e702ed1236b8e0e03e5e59c3ff05340be4025000000004c0a96f4f059cb19a758a0fa8519cd4f54a2a897be3280afc80819270c24f46291def0fe251497bc477f658b3ecdecbf943e7880aa874a6835ea56d1a28e078c76cca15ad8e365b661970afde09f667bb46fbefb7dd3a67bb597e6a0eb06c1792683d7712b3a69751453330b99b313251a5debbea1bc28ff6384c7b311cea8610295251493a45c00fbfcef37daa70a7db2a919191e700f49c95ab0f9410d80cb44c5506c04b173d963e7c4cd46d0b196a1e8b68078bd6ec55afd612de231137f325df335445bf7dfd23e8b3d4d9294a2f0855b3ecb93d11d3cea4db40fc7304b7149eb8f8ab8daed8069827b5f188f1350841e0a635aa77bc775aa54629930ef9dc34947302318674081aa9f5e72b299dd8da276ee79b48451ad5da4466bea088c0cf069be74b5b4a631aac2cddcc2f5044ed316be0ee2df6dbc876e76c835272c2df98ca6c6266a33d122ffe32e4072a5dda63561aefd7d3b05b8911e8c19573374ca0943d91aec4c1eb4eff5f504a5e997ee08227016797513f48d6f888060112e9b553a088f83dd6ebe546314ad7781ac79346ee7a94e71a0d1274fd0beec0facefae277a9f4add5dd92b8ad6f3d08171887ba554a50ccef59d90c130dbc1d6ac02530eeb7474bc9b6159a4cbbb98b1338f3f7da1cf3fb18aef2212d56ad8acfb8ef5cbf8871747db580a325a7097763f10d325d13f203b22efb0c0789773a705c51483979c4f244cf1028d302048b24614267ea51f60f4316d8ae6258cac37d449a1389a11b060945281b5dc7eb74a500da941a8a321adc6a3b65c3c61ee010aebde697ae114f58801c4acd5e71c4a27c717afadce4f5da646c71c159311c0fad12a5eabde808a813472b57710aea33ba79a3d08b06939b74e2541ff343d680cc0d533ea4e08f70c3a8b0a070e63b313679548fed7f7b4a86e3bac575e6ca1077b687f4360e941c79b6a6db6231d1aa7414e2b0b753053c34b226e78416fad63c93c67e8ea1ff7ea298d5983589c10a7ff5aa8dbf6a04b1647055d93720702f0b61c9b3b3e99dcf437e88fe92dd34ae09cb91509f04aaf8c0f1306ec3be3ba4008fbe7c7232cd6606fa9cedd19d37da7ed6055b66a6ebea65b5bcf9ad01102f6e75e66b8873beb6d3fe63ffa4bee7d4f29ed7a26f6d32a4d5290ae89d50d03fae20979ccf39bb42193bfad6d99440b30b186a90d8cef5cf7f34d3ff1a64c24dcb10c77115fea00feccec3f1fa4704c5a2db13ffc732ae3a5bbbf51fb5d842a86fbef02e31f4c2551da10de4ac1f57f44dc9ec69d6ee1999a20c5377665f3b5bbc4667904b3a6d2b945ee721dce6f5de0b4359ac683b8829c874e43c6da544f08c762ab4758a32f128dc7033e7754ab388defd3076cad6fde6bf334bfa8c219ae578a19a223b0d3098622711fb233b2b6e1cf6f5412594016f694155099f52bd8fbab7f61943cced6509ceea9ecf6f93c94cb9f82ec0f7dc83289e6c1fd5ea3bf51dcf1a0818129221a241f5e20167b2b3fed99613d430cb6e515e8ed6a1695f4cbd540134eb27301dd9b662421b2f074559cca02b3332e13aa982a8ba952ae2db2d25a805231d1747121c5d6a3ddc120dd91d0ff393dcac077857643386d85d5d5ba72eff32673ec0d8502435a7e3af3a8f6889522d15b70589a9d95db9afd63eb7108fb30b8fb4c2c4faa6cffeb143e012d1fb187a27900bf8ab383a800acc13f427de840aed138b645442227e6a1fe76759723cf5143134bb23b89abd240e2864c3e721d9cb8ee3cb595d5bafe1fcf2610dd37a6a5d0e2d11f579e6820e5fc7adf80b146ffd4b6f796d4107df7e4caf21a8448501c431e4af430523710fd04ad692b18d2781dc701cacb10ce4eb4dee7ccb1cf2aca49694fa098359fcf6334683a5ffcb625043b4416161568267f1376751293e5b43b900e99d63a2e9887af25113297cb72c2c807936d1a833bf4f3232ebb773551417d78c2fbd7f8c0acebf6c0f4f8cf67b806d164b358843b32a0651b34ecbf32fb3703a8cabab44b578ee580abc0789dfe8e64bd8fb188eced4d42bce0fbe37ccc9830fe670df9a233725aeb55c03e6ebbb129c4e0e016b420478d24944d08142d993071c5c21a39cb52249fd71b015a14cd1b15777282d60f16f6624c724fc44138491d6572ac0de45a026c4ee0e6031cae805aff7116c6d6ae220616d41b3288c725ded41fb87641960d398f0199940de330835cd1cea0d7cf786d3132620b4ce137f68f7a95601adf9e5758eb3d506116743a665f62986929c2550f296b567db927dca59a5a48225e2e2dab06faf4f59f68ddee40af9069f41cd4a01095b7337b66830806e159667d05f2e24d02b0ae9648fae40c9094dd18476004e0a1c58d6ae52622ba00b387a5ee641730f2d50e4f4c6f0c145030aa1a6d1729215db8bfd64d0ea558d3d069377f07f0c63ac5aae13e42e783c407a7e835b7f9d1930b13c908a3d99b589cb743ca78b623c159c632c8d9cd5e80532a344272f941755ab9ae0fcbc2ae912312aed500e6e97090af13456ef132bf6038c6c697cdd7b21d450f5f5cd5668ac7f11595efbb8456a709ecb94b4d9a2599ab919be40802602648377110e66933f3d3da6ae69d3c22feaea81bce96f5be9b8e76b91569d8f055e6a0ced220185f2bf45f3954d12f61bb003059609db0b578448ea78c3fad0d06adc8103ed4d83933c666f59679f46285364f1e387e584d52f1bb01cf30c864fc1b21c7845686315e71ab83f23d808b0c207b37d80dba1b5dfc5e63d18e0921fcc9b8c2e2d7cae0b0bff858db0115aaab262348e013782abb7c6261f9b2d56269e096bd2405ab26791855a76f109199b4150cf37162143ced9b94080d95cde83bb3e4e609ef1b2fd9bb50b821f17ca72e1f476f22e289cc731c4f55153ed92819a8577b6e32aaad6845f2403ee3f2b117676efa8e145f35e27ba69721862fed411ce9465b6caed913c51d7415a01b8e77eb974b81b73fbca9c818de7cd52b5ba92977aae6933aa149ddb7d7de4df769eba9e204f17230da8ed8764d0f5770a7a73024edf81cc4d99fbdceeb94dbeaa27ccce6342e31b73dbb5b69d03af57af6992b1528475f4a3207beb9a2eeb15145eca282a71569b389371954ffa6ba44b46405f413573bbfdf34f055332880f0f24c5025f555995ab4a584814432f0f7d2f1a19b28960ff939f35be6a65f530f3fa552dc020db145391105cbe4d7e1e5a4a381df5cc0608e64cee5210b18b582330305a5f41853997aa916488ad0ef5bc5199a05027ddb1d501cf3dbae3781fe90d2becc45901e9c0a7b61346b9a31450b0480e9c7a14bd03761d6f4ffec2864b857a303d068bfdaffffffffae2935f1dfd8a24aed7c70df7de3a668eb7a49b1319880dde2bbd9031ae5d82ffd601c8125f8292e98e0c962f1aa8edbe3a330abc87819b2ce6c5aa8b41951b091929fe561689c53f314cd85eea56f22913e00ca5f5d3197fe3bf63cce653f6d38f7bb39152d82fa33fc0c58228829d1adb7971928c71b5703feeb59c418fe96b49839cab11a59a198fa4f2ce38a49a062e60d86e3106b78469ab27e5cacb44e9bb5adb85d14687479b43011ecdc8e0d0003a58730f2bf5a233c11040df805f1994aba1737d9b695a52ccda45dad826aa6f66686a9e54fda89dee102a97ef06ca4990c14fd24b780700d2b755b9eea052be251f5b3f6d8086382787698e6ed76e70197b610b6041f57560841deaf538b1b683b0a01446702c9acf97bd3021c4f47bc89199a1f549332b5b0ba808b48435f60eec682ea99124b88efc104416680d9083aaa233885ced13255901ce657e81ec9ed2860d5e6813e14be894e49b246fc258a8bd622501cadd1af5ffcdeec786e53360eba49508f51f3f75fcec468c645b2935e3c4ceab4152090840807ea7fc7aa0bb344fd551fa94a5f18cf80960b435b89b02195232088a15edff576d20280e8de924c45785d4b86fa1c3dd7f50f015e0345dee5270759bcb0b108272ca3cfc11de5700c309eed13a527679df98661fe87e11c099b62658cb53eedcf7345fb0b129e3ab29be835f3e4010ab3ffec2d90be77dedee55bf00945b284e896bfcabdc5d356a1cfe5b8c541f967ba9774cc29b61b9665c976b1ed0dd6622bf5afce4ffd2741308eee9f0fcf91fe9af7f0d70f110a0bbcc47ba25423cbddd3131bd722f32080f89910cc4f052faa14e5aef42f20fa3d1985b0b6974f6f7204cc02627745c1b18da6429e9f32351b8ef8864912be5a039a4b4f561120fbb36e439055f58ae2c43533769ce0aa23525e85799dc98f01b2d7296f4a56a9d04e2449af157011ef3f2d815d6667bd0d1059d5eb75491d4dcb98b500d8d04f6397f54ce458c150078a4a7c453cd98ec9e695ee9677d2a44c1178fb70976a61473f04ed7225e51870e786f4489fc6dccc4e57c15ff5ee00fc2edd0be470feda15c17b604e39667e4f29cf219ea129b1dcd323140c9fab2c5a74241a3dcc7dcc2b3f74c60059a5f1d8e2584fe4242303ba8d4e8c5c5a2e08d3715460b8db05f8d0dd87633dd8501430276fab1608fcb2cac59af6f57615b2de846e98748584f7b923c6658dfafff86e1c0234fa5ca79a21b480a5e53da83efbfbedf5517211f5067959cf3f4e7f28cefb4c36b48f920743b9481489ebdb2687e8aba423c05f2c962349e95feee545fdcd4252cd219f65aff677ecef1a511113572e31912cd23dc328433751f979c36df8172096c52468792420e37184793e72d343c2d111298bda66c5588c8ae19133540330f5c1a7287fc8e0b096ff78a177e5f1205deb58afc8791c39310c33bf8d1405415667de10866d6db966ed0935532eeeb87f259c62946c6aa67d332188a3491b9e543ea8bfe0018b5ac92b312dd253bf97c79cbc7898304f07cd6d267cb87118a9b89fca94ff7163adee0ca11ac362843740d57d44f069e636e4760ddca1dd9686cf9c9165c33dba8bb8c460272565fd83658735a80e64817377c14d0134fd16c614abf55a75ffe3bfc3b8d9a3a185cb1ffa3c4e7642b0fd67d94aca3ca69ebcebd9718d44676dd3ad75b4ea99abd261f8c8998eb77f7ff1d31984e6abd69ab2c6ff3732e552006dab8b2146273d42783820e4c78fa3f06c9fcc7acc33b5f3e99ddd11cf0c811e919d0ec856bc05273b1e70f27538081712a4955ec48c8dca78875d1983e67a68c56381fa0ba210ed037296aebd423fa451c274d5cb8a7209d74ec26faf910bab1c49d4647b10f133b9e1b5d4cdba003121060aed316d9b0bd0783559fa4fffea072d2e2cfaa8b7bde4d460bf15852081c7f9ca794b22a34ed8a6114e7d8655299166a3767b8c9ab405f064291d6cadefe10783e2fc009e01e3d47f6ff8213eaac50a65dae91f7d02b6432db30d6f3a76e5138fc5206f7ae9e23eb167da3446517824736adb2e9127d39664f586dab8ad01a791427dc1979816774fffc55a1df3f11ceb36953d7e3fe1ed5d8b2a56a018f29e9c79ae7b848b055337b9d622d98ff46cd5290a15b4546304850407c74dc8c5a4b757b282e2f967764ec4dcd22188fc69af70bb0d1003bcce1f78505e69bc0e5a796718e95cdb54a6487b372cfc7fa3ee447ab7b7918f78d561385bf0da9692d2ea705c45ff166f8185828bf60659c28c49e99223aa877ecf85b0105c305a3d16f36fd2b07af98049f19f19e5ba2860d3595713a31e20fdba5f82b8ff4599afbd7a1d9e6eeff9cf36ad772235f4fa4e5171329018321fa5d4ee75b5652df53ba6151d991fe57403fdb851eb9fdec005acceb6159079e2d79dca07abeb18d3047fabb610697186109d3118f7ecdcda2a79133bca064e8de44cd0f67a873b3b3b86ce39600591a8f5dafd778c530a4583fc27d564293837e64688de211ba49630a71df3387162c7f438fb050591de458cb67f18690eccbb4d9ded51b954c461f7dba65125ee8276ff80ea28f4d8aace19fc6afbf7300ad5281d26fb957a3c75382f61f0b8c003726f363dedeb5b36dc10aa151a080ecc904ef83f8795eb780a3d068eef993d355ff10e982af039fec2d1811a13f70c92a3db6a5b99c90f5451cbc523d5cf8a53a26019e2fb64fbf8bac0277a0af51df20405eb751ba7a1cbdce7499dd2913cf896225643df881bc8596e9157603535e2db10c4c462b9406db2421666e6102d4d38d4c286bd892aade168343981430bafcd0a5552bc41b7cf078303124562f5065cd7be9e6891a384ac6efdf7a14e36a8af3e736fa737b769295a6142a4da0591c2e2764674a8ff168290fba3a2a225ce85f6514c3cde75f9becebae4a15ea3c8541ed9d91214a1d3d1c93d7eb4b718955e24f5bd8a3bc81b8ee7258c29c2cb880ca265af8fa0ad8b52b7bd37d3fa1cb884030f7e8996063d64f4b87f7b2ef33bbc109b8409e3034f880dff706a2b3082ecec4769ef86d3c0302e88dc2a9edd9ef5f9a09148207547ae8f3519131d088025c7117ec51bd8c5856a05a19f89607bbca7a64d4ee6f95d407847d862511b3db8b6ea6477b63e165e89eb8332ad02fdc7a45529d79bc5983b9938e9d5181d5b107b29cd69b9a3e3955a76304302410df295a71a8a1a1fd36b938d46d5bd27423671d87b1d06c2b6119037625abe62f8ffe7ad62bff7d702209f62c5fc72049bbfcff974463b4728729c341cb107ab6b6e9579be48bd66c20d6894d094f02b8af565eac58404da7396f46d0b9e7f700b7459bf8123eb81b84e7a1f5ec05c09cb633d5d02e2bfd6032ad23aef6e95bee7b1bde7c13ec268a919c2f5a1173a1f4abefdfeb377d32a5e242db005ca73cfaa967b984882f1c714adf632f5957c00ae0ba108ab5fdd8064c846c97ee99431fc341ed83a6eb4db0ae81aae60549328575cedbeda473526f4454ddd00318012bf95a67e6e0dc66f7e7505efd0dfff1e2812e8f2f6569dea98039142f0925dec54abb3931a4c902cdd141e2281d7503f350f463311683eb53d60350cd707591ea8776adf0a05994eabd90de191d7133771a9c3ff804f05cfaac67e55ad0dfae5f2379fd7f4e1cfb31cc20efd53efef318f16178473b32be9b304b0b4295ea5f9d19752dc6b381b0f8f0ccb0a54f0872ea2c206f7913a9c00b5caf32441535edc697f79395a777d965f53f2ba4eaa071d0db33a15e194c8f654aaf781d4942d8b1f1761cc887427b4833a53b024f2140c4db09c5480b32b5b1b5fe7c18b43587e9ff4554f49ca6df1661b5603587adf0f76c94e570e23763cce1141955e804839e284c4026f7b5bc51473ffd51d11773307d9489abd674ddfe6f0400886cfc2535bf71fed189af5e7838f22aca65eaa3c011e983e2285450e9fac869a31c4101889c4779d90b04f9100d23115f44e320b2abcef6529dc2984dcf6e98de1ecdaa7981192824be00f57d62334b48bb8fd210efacce5c2eb90059aee2f6aea3dfdf79ccdbb1df1dc58f549ba167b2fe4390d27cbe9b209d606eb4d7ac193e5b70bad4e3d127187b442e84a541f33ab6acb2acbfb1a63fe2126208e640fdd6fc705168a4cb714e7297ff2b6d6eeaa80d9a4114d5572e98385e68c88d3324cb21083c8d0c00bafd15c715317d6d65b8f6e40165b0d61c778198f85bcce277a62ec5f54e109ab716d18460deada76fff788bc34470ee67960e2b09a1216e994bf3a8dc77aff1ae089a881712fbd7d538313942760d69296cbbcbefff11003303c03f18f776afb5ca7bc0e2425cfa93aa144b92113b4924cddefef071d7398611cd7a5d5ed895ca6e23c93817feb8c396f58f32e3a19ecd8570fe3a3c7ce0ec387e872c8b6b48a46cf47f59d4d83b3a4ec7a7116ed963607fbd4ff1786e9b070f014c7ea0b8dd4a66548ca3f1018ad69fcd8c416a91e9fecb949602aa7669bcdfed72b174f1eef753d0e84fb73ff8786981b630e6567ebbf0922501474bffa1068602bf76e8d6ceef7279f504fbaebeb6a2b1f10031fa8ca81731d219d1f05cbdbfb0c6ee3b6ce8f5c2665aec2810cd2f053163011cf9dbd45edd0728a5cb4ddc5b8d7b3d9d858f7a1c6389404f14d81432c380ddc60d0e2d230e8dfc468b37087f6ba3859fad34da15880a7c4e07a8fe3e3820a7d1366b1e8f8e12580fbee20be7c0fa330f6e8b16263b54e0f09f22565e84229df32a7d020aa92a343b2f38c11a9d473fbd514aacc6c067f6411a60e0b1f59160d2916995e5ee45ad971682f39eb67f9db88e33629e3f3fbb0279f57b096b5015c1ba4bdf3827ee846e7be9b8a71c3db510c008503e9a0d8d642499556ae9a0782d49426c183147bf57a0a407b13e08b0bd275ee377aa28b9f14ff2035b54919fb3f0b82d98712fb9739ddad83922b454aa3ed58ec707808b226a8b41968aa00e356e40a1a944dedbfaff72c65a86d551bee39cc5f33d42e2245455f9b526d26208fd1c84587b22541b36432e78a18ac0fed46d7755b063037b90b5f62819a3761950d035329e3bc248fb26398653fa9e5df8f395ce0bb2a96dc21cd7a0f852d5c1ef2103c494724181404c24c6b47cf2007ea83f5b61a420d3ba88a54fa3d05a53755f3eb15ffc2749f9911f5ceb78d911a1bdd19ad20839fa4a454db081d10370ae9a85a83a6555bfed92cc66eea4a79e7a63de670770cf1046fc405e8123530f1555e28eee92d5288786aca0c9dd954aeb504071e6b9292111ace7a5315223ff6f13f751f149929503215156dd83f960f693d3aa89e618ab5cc67b556f735bdf1658324b54f1607eed7b7f22df7fd16b16fb7693d9f57fb8b55ebdbabea2934f5e7693829d6d92b9359ab82c248394c19c2202d0f96e7cf3574163e9f2b2cc2c29c15dac888ef7ebea7c44744f82b21ee0f48a43e98dc3b9109d13fccaa1b96a1a7bcf5158e37d47c06b6a0b571750da534a1f312de21694787dc4e55410ccf9e6323b3d8c5ebca8ed0bced8b2dfb38aa1d1ca6b11716452c9b378884792891e2923779fb38462adc3c67887892ca9691c17b35cb65a03179172a5d074919360c07148f225a3576b4710292cdb80beffcd5758a3df9d7d3a688bf7825cb0945da118f4c5a8f396c507473062177521a14cc5f9514d4adee65d176e16d06134d0f0bf36d08b2ca16ab1818b55f8e7fc06caf1864dae4191a17bc008d7cd624b46178274893be8f33b268abe0c75432a0e6789bd05cb67795087563994484231a159c34a715fe60c60aa36e46f4233bbf78e767171d42a5aef17f0a892314113f6da715d80b9daf97a256395a28b0a402a410371c0df1275e4dbe6ef649b43b151067c5ba656976417134fc2947971b3340137ab699b5f405d28c09f078ad10c7b6a2488cbe7dbc8acbe1cdcdc8c27ef4953fadfbcaafbd8072910d680fe6154ac6d6d3424d09087cc6c74261a6d34ae1123e68d4cd67c530ef7f35453f8f3baff02fd17def947bd624f744026a224341b428f7991d6dadbbb5d1083fb72225061e51f676016463b698c3f7946d8dbe2de4bbeeeb89dcb591606c81d198360b28f5fd7b50ee667fbe9b3b1252d468e6874d20822822f915d0f603554b8c1823d757ac63b9b6331255db222bd6b9f35b5996a34113b6baf556a02298039f7919a169c691ed07ecc8dbd52194c769fb51215e4dcd4c3b09324ac8c26655f697203288dba03ef9ab050be80be032af30b3bcf8c86b8b36f150a08da71be0a91510f5337c71fe8850ed3b796e8ec8d974a372905521600795555527f60f215e18078d55a3eef7a652921965fe052079c06a4b8e2b4a58ee0e814970c6b516f98a10dafcb4c501666af1e15714ba7032520d03c7cd793256f968af5c1b21291ca916315b2b608cf11e82f481f92421e9cd731430e49ac5439d3ac68bd91e7edcea067d9e47f8c0f8a42671295ebe9fb11f87a2a751222e54c85d5b48c1372ab31629b5f2e713fa12accfd0863f98c9c722c8e44816be7124b0db0b00ee6e55ad3231f5f1a800c57414e64c69db9180c1864e84c8319b10a5da17fbe9d19ce85fb530ba4121a9882ec877931f1a5092d1e39cafe95ae2605d11c2306a356bcd9b53255fb3aa4aa54b98355304d2eeb265e5450b21f83b2b7f64b68cd2494c89ecd20355665fa3ba57a84955c895643e552d90643261a16a62211c146fb5c59126341bfae77ef30e1452a759bc42baa42113f09fc271e93d56bb622bf0da0bd5208c30ed2f47abb7464f4354e8385295fdf20d4bf5c22e11f7bbd8f16056efd6627a035ae70ed514602c4368505321fd57027cde90ce7211777c4ae585447e6dd24619d1b2178ac2fba51e149cdcae213170649a35ad7f61f1e83283127ada81512e0862c780e712964bf98a5254bda758b58ce9f1418769662714541f3a7a6ecf97200e55fa27c422540cfe00f1302a4b45f4a39312babdec27e3b739b88ee231e50cb1959e8807edf26e61f297549f6e7935898cbc399b4dbf31b51af792bf6e4822d59f08d648c326036e84f835a58daa883b109d55a0db23e4325f2245158c3f085acd7f90f46d8ec9709591b0bb7cafbc5bb6982ecc29d1f80a91ea66c4725b1d538346ba92b2583d3f2954cb070a5d186c9587997aff38eaf82aa24ec5bd6d2be7d1f01f3f21412333ef80dedb2bc95842ddceef5a4ecded8f8be9d1506d113710750c80f9a0fb13f99c3cbc19fca04ed698c188c0413806fbc00a4c8a0b89312b93ac0b960ab061e3dac5c53d26f27de379811cf37edbb18daebd40896ba33129be2eb1745376e4818cffda9526591d9f1cae23292fbcf67b95aa8685f6a73b2e4e594df8c401b99b85d566bd9607c8ed3d9ad320771572fbc79501a9268304feb9eac4b0258ed8cc8cfb3675a4506c93c46c35285204812be067a59252ef262bdb688f48433e57e05e969d81e2c00359da27c6e766d8ffafe5f56008b04d152005159a7dc1981c4b05a81ffb40b90ccf6ca7b68f383333799708e8ab0d65249c6c579d11a1ee68c9e2a30cc1046577cbff2be5650c68e9006366e252bc59325b24a85d029a7192050b779783bfa6eda7b3f6d9589c88e219a76cc0866ab610e128d67325fcfe03b8310a08ddb84e5bfe056d7df760c98599b064817dcd52225b229fbc382aa318bec944c9ea07dbd9ca984876a5741672a4f8a2e0d7d391000bdac01233ad47beb3a03356f28c9386aeef3dfed6c829109f8aa27774f6cc3fc83c43740667e1ae11fb5fa75202686fc904194399cc5a0a137c89c2d3098c1f5124ce916cd488fc6f03ad68f15117dc7384737025bd9c98030253f740476c38eafce3a128663ae8aad66f60a43353fe57f51e589e470d7fa6920f5747f5760418d374da7b77f2eb3850d81eb5789a543fae528360db30808225d733e227ac1adb5370a7895667322cd7b814e6fc21ee36226f93ff61b145b5e9dad06445c13f72b763dcf0104ee7dd3f9d72cea4a44416bf240448929bd51b76074579677518409c2d67a64b285ec9ef104d8b79c785720e85bcd3278ead7b52e2b3cacba633978818c4f6dfab9834217381ccad354b672386f9974630b4a829770d56c283272f0940c52434930594a1d00bfad5764d69fa0d6149032eb46f41e96239a2930b19226ecd32ee6bd84d974cf80a6f6b810bc10dccb5c4aa41a56ba1f0366992153ec5e543d1e96593c7544d61ba8584e54401d2bd2910989151d610fe03f8682d27c4563e7430b7cd9d9ed0fbfe4a16981a4fbeec074785943864f5308fe95226e48c51e8303f4cf6e34ed4303fe6d08f3a80d0d8e8cc5e5b06e9628210e13c264cf78d87a1d770112a5109121adc69b494922dc5655869672e85a39b9188a52f005e11f48a10409066b270f31673201a088b3e7ffc67aaf2aa021ecae704320ccb425c686be25c9e470383945ff30583fb24c1f77e6453235902d308d93fdd3d17fcc2cb92a51ff1e005d24eb91699ccb5cb6cc0fd95102eb1e53a279c4bd009d9bc4c2c74eb8c69c8b9474947212c3d2942d790e1d3fc1da205315e51a2420212970f330be90e5eeffb99cd59ee4ac00dfdfc56cbb07359bd1d89613434e910467ad02d889740b2465c39c102d613a4fd9665bd24dce85d23481fe0f96fdd0d738a98a27bbdf258a3b638df2f5ac519c9cb3db884c7babd36936a30070bf53bc6604749740b63f392c3c486e1c2d5b30dfcaef9a59ef461d52853e1ed9b7c0878c8d4eaf31bdf2cf07178b1e8b9ad3191591670c9836fb74c331bc322bfb43d3cb3107de5888086ad0f1da971a5ebd5f7006eb67437f00875d13d1e9654a43a0e2b6c3fe1ed485e3d8d973eb8627fe0c54a2d5ce224f507c05981452ee432b212c13c957c8a62bd021961f4d2a6e29e4d2970718b845e95fc84f4246dd206161f5e89d1058fee99db730fcebd2e80e4ea0da745fe5d938f7cc540cdaa0b761ef0dce753972ef3cc93a0991af04972b978a16111adce36fa5d313cc74a8b5f19e3f90dac7038a801b14e57ddf3f0c462177fc351a397de21fa4c77d16c0d25344ec6bd51c767c51c89dba456dd30a0c5ff7ca064f069a9205db6955f47aedc2fdfa6e50bb562d8f25f9689a8d9a7b746513bc40c8d1f8241d36a64793bfdfd32e44264ed3fe727fae90f53b4c7d1ab81b8efd1ef5ecbab086a64040f29986b0f50d5e737708753462aaa60e5daaba93546e14b7f5881c7764d2770ec1be1033c8d7b12a7fddda3f8c5e7b971a75e6d65c130ff71de698292af4b00c70cd4580518377d877426cca3476e22707221b43817ac047cc1218790d15791d9ef996e2e62716eb61af16fc661639a15b36c67195ac3bd7267eae7d627b559bdbc03ccac627190f04d17a95a5b0dbb6b3d1954649050a0f8f48ac0cb03b75eaf2faf5791684a6ff5b15ad675726801996cab891888cd9f52f552a917dba227f7872d1da89e6a9a66c97d0c245bf452e4f944c45f6e6e415adda76dd601cddd9ae9306481580c79f6b878dd8151db641e32321c6253b743bc9b16e16c17d59c8e6465050e026db9acf49e90f9fba44baa027e341c0a137a224552085dfa436b9d9ed72c9e8724f18ffe2da6e58d3a8053c31365e895753bb2fdcbc345d643885ddf70cdbcf1f7ed85785048d36e9094e2b560a33d995222e89aefa058dacc5abfe9ca17b343dc6bf7a65d5807431feb7fc8b1736815909ab5cba9f45072ce31a11c912c05c80a2c9a9aff3104575f51cadd41006a3e8f213e384e20be2b53ac1dac8e33af8c9319cbfdfa4be155a754dff819bdaff9de95659ef76b29c2ec2baa3b76c196e44e6a66b21a303614c6428777d567b3dbab1208045c9781d6d9de4035d90bbb920389cbc0d302b14279dfe3dfc1b38b145dfd750ddede8c7b5647ced0a5d1bb10401696cadefd4acee78a4b0ac615a76602daea856ac38fc7d63ae875c82d9009099d66e25adb6710843f7a1d76a05b807c36e218f29042ceb39c9a114dcabde2f7c61c60adb39077535dd03719c3a0f8f29e43d833d1405b5bd14bd864ab85b4e82db1e3b188484539379914bb207e97bdb590c9b72e3d7fe2da3bf07da6252e7c3b76d8a7dbba138c2ca05d06d0e6f80fb17519467c094973300e024a5e90ed05fb52d71ae7619905e50ff1dde2767d1fd2e4334cf8d60613775b169e3a068d9361d3983f48ef7f06e1a6230a5b1aca2296e0d234c3f9982c5591b54aaf39da981bbb9f21c3f7d2b6cea2e982e2d48e8b42a4b462499f674a8d1b6cde4a3b7f4f8797d8044e59c938c63fa4cfdc1beb0d28076c43fc68ffb90b7995c0d80cb502510c99cdf3f0ad1978569b6bf7e91679b4cf8d4ebd476b46e7cb3e8bde14139ea40934a7c134d6585590145f8396f2436dbf5099bd480b32c16bc1fbd305ca9083073d28705baa520a354ccae7a0aef51ba2d3c93e58cba9394173d76e8e5a0d701d97b7b4f9daf9aa1cb0f04efe063cb5575e99e26dbe691d732fc11629";

pub fn first_tree_state() -> TreeState {
    TreeState {
        network: String::from("regtest"),
        hash: "0543e665ee9d1155fb0f9700c43f22f87e527\
    070734bee1ef015dd14039436df"
            .to_string(),
        height: 1,
        time: 1686238700,
        sapling_tree: "015fcb897f9314fc5ebac4510b86706f45458c6db1d\
    36dbab6ee23315e9654c54c0000"
            .to_string(),
        orchard_tree: String::from(""),
    }
}
