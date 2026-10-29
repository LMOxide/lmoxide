# 12c. Rebuilt conversations to check against footage

[← tracker](README.md)

The story conversations rebuilt from the client's texts (content/dialogues tagged `reconstructed`, tools/content/story_dialogues.py; evidence in each record's `comment`, docs/TUNABLES.md "Rebuilt conversations"). Tick one when footage confirms the order of its lines, where it starts and who speaks; test with `!goto rebuilt n` or `!dialog <first text>`.

**0/64** checked

<details><summary>Dragoon Dwelling (0/1)</summary>

- [ ] **Cinematic Rescue Revolutionary Soldier** `rescue-revolutionary-soldier-778` — pf 1632 (141.7, 57.8, 96.85); starts: area Cine_Rescue_Revolutionary_Soulder1; texts 778, 780, 849, 850, 851, 852, 853, 782, 779; likely; speakers: Revolutionary Soldier, Swashbuckler, startfxpackageonactor RevoSoldier 239914, startfxpackageonactor RevoSoldier 239915, Revo Soldier, Island Warrior (window: Cinematic Rescue Revolutionary Soldier) — check: line order, trigger spot, speaker <!-- rebuilt/rescue-revolutionary-soldier-778 -->

</details>

<details><summary>Jacket Cellar (0/1)</summary>

- [ ] **Cinematic Rescue Pizza Guy** `rescue-pizza-guy-783` — pf 1670 (245.86, 13.43, 265.83); starts: area Cine_Rescue_Pizza_Guy1; texts 783, 784, 785, 786, 860, 861, 862, 787, 788; likely; speakers: Pizza Guy, Swashbuckler, startfxpackageonactor PizzaGuy 239914, Revolutionary Soldier (window: Cinematic Rescue Pizza Guy) — check: line order, trigger spot, speaker <!-- rebuilt/rescue-pizza-guy-783 -->

</details>

<details><summary>Isle of Yarr (0/5)</summary>

- [ ] **Cinematic Start Pizza Delivery To Fort** `pizza-delivery-to-fort-811` — pf 4200 (231.74, 64.34, 280.49); starts: area Cine_PizzaDeliveryTrigger1 (needs tags 4143); texts 811, 857, 808, 807, 858, 859, 809, 810; likely; speakers: Pizza Guy, Guard, Jacket Guard, Pizza Delivery Guy, Swashbuckler (window: Cinematic Start Pizza Delivery To Fort) — check: line order, trigger spot, speaker <!-- rebuilt/pizza-delivery-to-fort-811 -->
- [ ] **Cinematic Start Pizza Delivery To Fort** `pizza-delivery-to-fort-turned-away-801` — pf 4200 (231.74, 64.34, 280.49); starts: area Cine_PizzaDeliveryTrigger1 (not with 4143); texts 801, 805, 803, 804, 802; guess; speakers: Guard, Island Warrior (window: Cinematic Start Pizza Delivery To Fort) — check: line order, trigger spot, speaker <!-- rebuilt/pizza-delivery-to-fort-turned-away-801 -->
- [ ] **Cinematic Start Dreadleg Escapes Again** `dreadleg-escapes-again-828` — pf 4200 (311.17, 69.43, 318.97); starts: area Cinematic_Dreadleg1 (needs tags 4161); texts 828, 829, 830, 831, 833, 841, 834, 836, 840; likely; speakers: Dreadleg, Swashbuckler, Evil Pirate Captain Dreadleg (window: Cinematic Start Dreadleg Escapes Again) — check: line order, trigger spot, speaker <!-- rebuilt/dreadleg-escapes-again-828 -->
- [ ] **Cinematic Start Dreadleg Appears** `dreadleg-appears-382` — pf 4200 (373.81, 68.28, 347.41); starts: area BA_Cine_DreadlegAppears1; texts 382, 383, 384, 385; likely; speakers: Evil Pirate Captain Dreadleg (window: Cinematic Start Dreadleg Appears) — check: line order, trigger spot, speaker <!-- rebuilt/dreadleg-appears-382 -->
- [ ] **Cinematic Start Welcome to Pirate Town** `welcome-to-pirate-town-386` — pf 4200 (437.09, 68.24, 159.31); starts: area BA_Cine_WelcomeToPirateTown1; texts 386, 387, 843, 845, 863, 864, 865, 846, 394; likely; speakers: Swashbuckler, Revo Soldier, Island Warrior (window: Cinematic Start Welcome to Pirate Town) — check: line order, trigger spot, speaker <!-- rebuilt/welcome-to-pirate-town-386 -->

</details>

<details><summary>The Peglegged Mermaid Tavern (0/1)</summary>

- [ ] **Cinematic Start The Pegleg Mermaid** `the-pegleg-mermaid-155` — pf 4620 (227.16, 42, 173.63); starts: area BA_Cine_ThePeglegMermaid1; texts 155, 157, 866, 867, 868, 869, 166, 167, 870, 380, 381, 170, 171, 172, 173, 174, 175, 177, 156; likely; speakers: Good Pirate Captain Pete, Pirate yarr, Tavern Pirates, Swashbuckler, Island Warrior, RevoSoldier, Tavern Pirate (window: Cinematic Start The Pegleg Mermaid) — check: line order, trigger spot, speaker <!-- rebuilt/the-pegleg-mermaid-155 -->

</details>

<details><summary>Pirate Trials (0/2)</summary>

- [ ] **Cinematic Start Now we must pirate fight** `now-we-must-pirate-fight-193` — pf 4300 (84.98, 35.75, 271.05); starts: area BA_Cine_NowWeMustPirateFight; texts 193, 195, 194, 197, 199; likely; speakers: Good Pirate Captain Pete (window: Cinematic Start Now we must pirate fight) — check: line order, trigger spot, speaker <!-- rebuilt/now-we-must-pirate-fight-193 -->
- [ ] **Cinematic Start Completed The Trials** `completed-the-trials-263` — pf 4300 (89.83, 40.77, 255.26); starts: after defeating Captain Pete; texts 263, 395, 264, 265, 396, 397, 398, 399, 400, 401, 403, 402, 871, 266; guess; speakers: Good Pirate Captain Pete, Swashbuckler, Captain Pete, Island Warrior (window: Cinematic Start Completed The Trials) — check: line order, trigger spot, speaker <!-- rebuilt/completed-the-trials-263 -->

</details>

<details><summary>Volcano Island (0/1)</summary>

- [ ] **Cinematic Start Dreadleg Volcano** `dreadleg-volcano-404` — pf 4210 (190.51, 84.22, 201.81); starts: area BA_Cine_DreadlegVolcano1; texts 404, 405, 406, 407, 408; likely; speakers: Evil Pirate Captain Dreadleg (window: Cinematic Start Dreadleg Volcano) — check: line order, trigger spot, speaker <!-- rebuilt/dreadleg-volcano-404 -->

</details>

<details><summary>Pirate Grotto (0/4)</summary>

- [ ] **Cinematic Start Into the Skull** `into-the-skull-200` — pf 4310 (280.88, 41, 167.75); starts: area BA_Cine_IntoTheSkull1; texts 200, 201, 203, 204; likely; speakers: Evil Captain Dreadleg (window: Cinematic Start Into the Skull) — check: line order, trigger spot, speaker <!-- rebuilt/into-the-skull-200 -->
- [ ] **Cinematic Start Captain Dreadleg** `captain-dreadleg-127` — pf 4310 (356.52, 56, 124.83); starts: area BA_Cine_Dreadleg1; texts 127, 128, 131, 132, 134; likely; speakers: Evil Pirate Captain Dreadleg, Captain Dreadleg dizzy (window: Cinematic Start Captain Dreadleg) — check: line order, trigger spot, speaker <!-- rebuilt/captain-dreadleg-127 -->
- [ ] **Cinematic Start Dreadleg Defeated** `dreadleg-defeated-259` — pf 4310 (384.26, 58, 157.32); starts: after defeating Captain Dreadleg (needs tags 316); texts 259, 260, 261, 873, 872, 874, 875, 876, 877, 878, 879, 262; guess; speakers: Swashbuckler, Dreadleg, IslandWarrior (window: Cinematic Start Dreadleg Defeated) — check: line order, trigger spot, speaker <!-- rebuilt/dreadleg-defeated-259 -->
- [ ] **Cinematic Start Dreadleg Defeated** `wizard-calls-the-compass-880` — pf 4310 (387.75, 58, 154.66); starts: after dialogue 259 (dreadleg-defeated-259); texts 880, 881, 882, 883, 884, 885, 886; guess; speakers: Swashbuckler, Wizard (window: Cinematic Start Dreadleg Defeated) — check: line order, trigger spot, speaker <!-- rebuilt/wizard-calls-the-compass-880 -->

</details>

<details><summary>LEGO Center (0/4)</summary>

- [ ] **Video Game Guy** `medieval-world-news-267` — pf 1200 (286.64, 46.25, 73.71); starts: area BA_Cine_GamerQuestgiver1; after the cinematic after-pirate-world (needs tags 316; not with 437); texts 267, 268, 468, 270, 269, 464, 271; likely; speakers: Gamer (window: Video Game Guy) — check: line order, trigger spot, speaker <!-- rebuilt/medieval-world-news-267 -->
- [ ] **Video Game Guy** `space-world-news-466` — pf 1200 (286.64, 46.25, 73.71); starts: area BA_Cine_GamerQuestgiver1; after the cinematic after-medieval (needs tags 437; not with 492); texts 466, 465, 469, 467; likely; speakers: Gamer, SpaceMiner (window: Video Game Guy) — check: line order, trigger spot, speaker <!-- rebuilt/space-world-news-466 -->
- [ ] **Video Game Guy** `mythology-world-news-530` — pf 1200 (286.64, 46.25, 73.71); starts: area BA_Cine_GamerQuestgiver1; after the cinematic after-space (needs tags 492; not with 676); texts 530, 531, 533, 532, 576; likely; speakers: Gamer (window: Video Game Guy) — check: line order, trigger spot, speaker <!-- rebuilt/mythology-world-news-530 -->
- [ ] **Video Game Guy** `dino-world-news-633` — pf 1200 (286.64, 46.25, 73.71); starts: area BA_Cine_GamerQuestgiver1; after the cinematic after-mythology (needs tags 676; not with 909); texts 633, 629, 630, 631, 634; likely; speakers: Gamer, DINO WORLD (window: Video Game Guy) — check: line order, trigger spot, speaker <!-- rebuilt/dino-world-news-633 -->

</details>

<details><summary>Kingdom (0/3)</summary>

- [ ] **Cinematic Start The Kings Court** `the-kings-court-272` — pf 5200 (115.45, 23.58, 266.39); starts: area BA_Cine_TheKingsCourt1; area BA_Story_TeleportToThroneRoom_CinematicFailed; after the cinematic medieval-intro; texts 272, 273, 274, 275, 416, 417, 418, 419, 276; likely; speakers: Wizard, King, King sobbing (window: Cinematic Start The Kings Court) — check: line order, trigger spot, speaker <!-- rebuilt/the-kings-court-272 -->
- [ ] **Knight** `kingnapped-278` — pf 5200 (95.85, 11.68, 175.76); starts: area BA_Cine_CityGuards1 (needs tags 425); texts 278, 280, 281, 420, 421, 422, 423, 424, 425, 277; likely; speakers: Knight1, Knight2 (window: Knight) — check: line order, trigger spot, speaker <!-- rebuilt/kingnapped-278 -->
- [ ] **Sir Loin** `sir-loins-checkpoint-426` — pf 5200 (332.46, 20, 138.36); starts: area Story_WallCheckpoint1 (needs tags 426); texts 426, 427, 887, 428, 429, 430, 431, 432, 433, 434; likely; speakers: Sir Loin, Wiizard, Sneak Walk (window: Sir Loin) — check: line order, trigger spot, speaker <!-- rebuilt/sir-loins-checkpoint-426 -->

</details>

<details><summary>Ruined Fort (0/3)</summary>

- [ ] **Cinematic Start The Realm of Jest** `the-realm-of-jest-283` — pf 5300 (149.11, 42, 116.39); starts: area BA_Cine_Placeholder1; texts 283, 284, 285, 282; likely; speakers: The Jester (window: Cinematic Start The Realm of Jest) — check: line order, trigger spot, speaker <!-- rebuilt/the-realm-of-jest-283 -->
- [ ] **Cinematic Start Working for the Enemy** `working-for-the-enemy-436` — pf 5300 (348.94, 45.96, 291.22); starts: area BA_Cine_WorkingForTheEnemy1; texts 436, 437, 439, 440, 441, 442, 443, 444, 445, 446; likely; speakers: Elementius, The Jester (window: Cinematic Start Working for the Enemy) — check: line order, trigger spot, speaker <!-- rebuilt/working-for-the-enemy-436 -->
- [ ] **Cinematic Start My Kingdom for a Crown** `my-kingdom-for-a-crown-288` — pf 5300 (348.6, 50.27, 315.78); starts: after defeating The Jester (needs tags 562); texts 288, 289, 291, 447, 888, 448, 449, 450, 889, 890, 891, 451, 892, 893, 452, 287; guess; speakers: The Jester, Wizard, Jester whimpering, Jester (window: Cinematic Start My Kingdom for a Crown) — check: line order, trigger spot, speaker <!-- rebuilt/my-kingdom-for-a-crown-288 -->

</details>

<details><summary>Enchanted Forest (0/2)</summary>

- [ ] **Fairy Queen** `the-faerie-queen-293` — pf 5210 (285.98, 49.25, 153.25); starts: area BA_Cine_TheFaerieQueen1; texts 293, 294, 295, 296, 453, 895, 894, 454, 292; likely; speakers: Flora, Wizard, Jester (window: Fairy Queen) — check: line order, trigger spot, speaker <!-- rebuilt/the-faerie-queen-293 -->
- [ ] **Cinematic Start Crossing the Barrier** `crossing-the-barrier-314` — pf 5210 (285.98, 49.25, 153.25); starts: area BA_Cine_TheFaerieQueen1 (needs tags 433; goals 3UX1); texts 314, 316, 896, 317, 318, 319, 320, 321; likely; speakers: Flora, Jester, Fairy Queen cast spell (window: Fairy Queen) — check: line order, trigger spot, speaker <!-- rebuilt/crossing-the-barrier-314 -->

</details>

<details><summary>Sorcerer's Keep (0/2)</summary>

- [ ] **Cinematic Start Danger at the Bridge** `danger-at-the-bridge-302` — pf 5310 (342.22, 66.27, 325.47); starts: area BA_Cine_Placeholder1; texts 302, 304, 305, 306, 455, 456, 303; likely; speakers: Elementius, Elementius chanting a spell (window: Cinematic Start Danger at the Bridge) — check: line order, trigger spot, speaker <!-- rebuilt/danger-at-the-bridge-302 -->
- [ ] **Cinematic Start Defeated Elementius** `defeated-elementius-308` — pf 5310 (356.33, 67.22, 338.93); starts: after defeating Pieromancer Elementius; texts 308, 310, 311, 312, 313, 457, 458, 459, 460, 461, 462, 463, 897, 898, 899, 900, 309; guess; speakers: Wizard, Jester, King, Elementius (window: Cinematic Start Defeated Elementius) — check: line order, trigger spot, speaker <!-- rebuilt/defeated-elementius-308 -->

</details>

<details><summary>Space Colony (0/5)</summary>

- [ ] **Cinematic Start Space S03** `space-s02-briefing-327` — pf 3200 (29.27, 142.14, 12.25); starts: area BA_Cine_S02; after the cinematic space-intro; texts 327, 329, 486, 328; likely; speakers: Sergeant Thrace (window: Cinematic Start Space S03) — check: line order, trigger spot, speaker <!-- rebuilt/space-s02-briefing-327 -->
- [ ] **Cinematic Start Space S04** `space-s03-mean-soldier-487` — pf 3200 (480.16, 116.53, 84.92); starts: area BA_Cine_S03_MeanSoldier1; texts 487, 488, 491, 529, 901, 489; guess; speakers: Mean Soldier (window: Cinematic Start Space S04) — check: line order, trigger spot, speaker <!-- rebuilt/space-s03-mean-soldier-487 -->
- [ ] **Cinematic Start Space S06** `space-mercs-at-mines-495` — pf 3200 (192.44, 123.97, 285.14); starts: area BA_Cine_MercsAtMines1; texts 495, 497, 903, 496; likely; speakers: Shadrack Circuit-burn, Merc Miner (window: Cinematic Start Space S06) — check: line order, trigger spot, speaker <!-- rebuilt/space-mercs-at-mines-495 -->
- [ ] **XX_Cinematic Start S4** `space-meet-the-space-miner-334` — pf 3200 (250.31, 126, 407.83); starts: area BA_Cine_S07; texts 334, 333, 498, 499, 904, 905, 906, 908, 909, 910, 335; likely; speakers: Space Miner, Sergeant Thrace, Pizza Guy (window: XX_Cinematic Start S4) — check: line order, trigger spot, speaker <!-- rebuilt/space-meet-the-space-miner-334 -->
- [ ] **XX_Cinematic Start S5** `space-help-fear-337` — pf 3200 (354.85, 123.56, 355.3); starts: area BA_Cine_S08; texts 337, 338, 911, 912, 913, 500, 501, 514, 907, 336; likely; speakers: Space Miner, Galaxy Trooper, Railhead Rangers (window: XX_Cinematic Start S5) — check: line order, trigger spot, speaker <!-- rebuilt/space-help-fear-337 -->

</details>

<details><summary>Space Command HQ (0/2)</summary>

- [ ] **Cinematic Start Space S05** `space-hq-preboss-330` — pf 3300 (255.65, 32.8, 203); starts: area BA_Cine_Preboss Fight1; texts 330, 332, 492, 493, 494, 523, 331; likely; speakers: Shadrack Circuit-burn, Queen, Standing at attention, The Queen, BeamUp, Shadrack (window: Cinematic Start Space S05) — check: line order, trigger spot, speaker <!-- rebuilt/space-hq-preboss-330 -->
- [ ] **XX_Cinematic Start S3.5** `space-hq-thrace-363` — pf 3300 (274.81, 36.1, 222.86); starts: after defeating its world's boss (needs tags 485); texts 363, 362, 902, 364; guess; speakers: Sergeant Thrace (window: XX_Cinematic Start S3.5) — check: line order, trigger spot, speaker <!-- rebuilt/space-hq-thrace-363 -->

</details>

<details><summary>Planet's Dark Side (0/3)</summary>

- [ ] **XX_Cinematic Start S6** `space-science-facility-361` — pf 3210 (116.5, 491.08, 67.27); starts: area BA_Cine_S09; texts 361, 359, 502, 914, 915, 916, 917, 918, 360; likely; speakers: Space Miner, Elementius, spin and point (window: XX_Cinematic Start S6) — check: line order, trigger spot, speaker <!-- rebuilt/space-science-facility-361 -->
- [ ] **XX_Cinematic Start S8** `space-black-hole-343` — pf 3210 (369.17, 115.03, 437.87); starts: area BA_Cine_S10; texts 343, 344, 369, 370, 371, 924, 925, 926, 927, 928, 929, 930, 931, 932, 933, 934, 342; likely; speakers: SpaceMiner, Alien Trooper, Galaxy Trooper, Space Miner (window: XX_Cinematic Start S8) — check: line order, trigger spot, speaker <!-- rebuilt/space-black-hole-343 -->
- [ ] **Cinematic Start S09.5** `space-builder-ruins-919` — pf 3210 (111.71, 499.2, 187.34); starts: area BA_Cine_S095; texts 919, 921, 922, 923, 920; likely; speakers: Space Miner, Galaxy Trooper (window: Cinematic Start S09.5) — check: line order, trigger spot, speaker <!-- rebuilt/space-builder-ruins-919 -->

</details>

<details><summary>Shadow Centurion Base (0/4)</summary>

- [ ] **Cinematic Start S11** `space-shadrack-503` — pf 3310 (232.4, 32.11, 201.25); starts: area BA_Cine_S11; texts 503, 504, 935, 936, 937, 938, 939, 505; likely; speakers: Shadrack Circuit-burn, Space Miner (window: Cinematic Start S11) — check: line order, trigger spot, speaker <!-- rebuilt/space-shadrack-503 -->
- [ ] **Cinematic Start S12** `space-victory-over-shadrack-506` — pf 3310 (208.09, 41.1, 212.3); starts: area BA_Cine_S12 (needs tags 592); texts 506, 507, 508, 940, 941, 509; likely; speakers: Space Miner, Alien Trooper, Galaxy Trooper (window: Cinematic Start S12) — check: line order, trigger spot, speaker <!-- rebuilt/space-victory-over-shadrack-506 -->
- [ ] **Cinematic Start S13** `space-alien-queen-349` — pf 3310 (335.29, 9.89, 358.92); starts: area BA_Cine_S13; texts 349, 350, 510, 942, 511, 943, 512, 513, 522, 944, 348; likely; speakers: Queen, Commander Z, Space Miner, Fly (window: Cinematic Start S13) — check: line order, trigger spot, speaker <!-- rebuilt/space-alien-queen-349 -->
- [ ] **Cinematic Start S14** `space-the-queen-falls-353` — pf 3310 (348.75, 14, 385.5); starts: after defeating Alien Queen (needs tags 491); texts 353, 351, 945, 946, 376, 377, 515, 516, 518, 519, 520, 949, 521, 947, 948, 950, 352; guess; speakers: Thrace, Space Miner, Soldier1, Soldier2 (window: Cinematic Start S14) — check: line order, trigger spot, speaker <!-- rebuilt/space-the-queen-falls-353 -->

</details>

<details><summary>Mythology Mountains (0/3)</summary>

- [ ] **Zeus** `myth-g01-zeus-572` — pf 2210 (29.98, 14.07, 215.13); starts: area Cine_G01_Cutscene1; after the cinematic mythology-intro; texts 572, 573, 951, 574, 953, 575; likely; speakers: Zeus (window: Zeus) — check: line order, trigger spot, speaker <!-- rebuilt/myth-g01-zeus-572 -->
- [ ] **Commoner** `myth-g02-party-guest-577` — pf 2210 (30.1, 16, 243.91); starts: area Cine_G02_Cutscene1; texts 577, 578, 579, 580; likely; speakers: Party Guest (window: Commoner) — check: line order, trigger spot, speaker <!-- rebuilt/myth-g02-party-guest-577 -->
- [ ] **XX_Cinematic Start G03** `myth-g03-grey-sisters-534` — pf 2210 (46.34, 48.86, 345.9); starts: area Cine_G03_Cutscene1; texts 534, 535, 536, 537, 538, 539, 540, 541, 542, 957, 954, 958, 955, 959, 545; likely; speakers: Deino, Pemphredo, Stir, Enyo, Wizard (window: XX_Cinematic Start G03) — check: line order, trigger spot, speaker <!-- rebuilt/myth-g03-grey-sisters-534 -->

</details>

<details><summary>Medusa's Lair (0/2)</summary>

- [ ] **XX_Cinematic Start G04** `myth-g04-medusa-595` — pf 2300 (196.8, 8.14, 257.31); starts: area Cine_G04_Cutscene1; texts 595, 615, 596, 597; likely; speakers: Medusa (window: XX_Cinematic Start G04) — check: line order, trigger spot, speaker <!-- rebuilt/myth-g04-medusa-595 -->
- [ ] **XX_Cinematic Start G05** `myth-g05-battle-goddess-546` — pf 2300 (221.51, 4.18, 259.38); starts: after defeating Medusa; texts 546, 547, 548, 962, 963, 550, 964, 551, 552, 553, 554, 965, 617, 966, 555; guess; speakers: BattleGoddess, Swashbuckler, Deino, Here is where BG should break out of stone, RunAway, Space Miner, Pemphredo (window: XX_Cinematic Start G05) — check: line order, trigger spot, speaker <!-- rebuilt/myth-g05-battle-goddess-546 -->

</details>

<details><summary>Armory (0/3)</summary>

- [ ] **XX_Cinematic Start G06** `myth-g06-puzzle-lock-556` — pf 2310 (293.13, 40, 397.78); starts: area Cine_G06_Cutscene1; texts 556, 557, 558, 559; likely; speakers: BattleGoddess (window: XX_Cinematic Start G06) — check: line order, trigger spot, speaker <!-- rebuilt/myth-g06-puzzle-lock-556 -->
- [ ] **XX_Cinematic Start G07** `myth-g07-thunderbolt-560` — pf 2310 (303.21, 35.55, 403.24); starts: area Cine_G07_Cutscene1; texts 560, 561, 562, 563, 564, 565, 566, 567; likely; speakers: Thunderbolt, BattleGoddess, Zeus, Key appears, Key disappears (window: XX_Cinematic Start G07) — check: line order, trigger spot, speaker <!-- rebuilt/myth-g07-thunderbolt-560 -->
- [ ] **XX_Cinematic Start G08** `myth-g08-the-key-568` — pf 2310 (305.15, 41, 410.47); starts: area Cine_G08_Cutscene1 (needs tags 671); texts 568, 569, 960, 961, 570, 618, 571; likely; speakers: Battle Goddess, Wizard, BattleGoddess (window: XX_Cinematic Start G08) — check: line order, trigger spot, speaker <!-- rebuilt/myth-g08-the-key-568 -->

</details>

<details><summary>Underworld (0/4)</summary>

- [ ] **XX_Cinematic Start G09** `myth-g09-the-trap-581` — pf 2320 (104.31, 63.02, 250.14); starts: area Cine_G09_Cutscene1; texts 581, 582, 583, 972, 973, 974, 584, 585, 586; likely; speakers: BattleGoddess, Enyo, Deino, Pemphredo, SpectralDeino (window: XX_Cinematic Start G09) — check: line order, trigger spot, speaker <!-- rebuilt/myth-g09-the-trap-581 -->
- [ ] **Cinematic Start Charon** `myth-g10-charon-587` — pf 2320 (186.7, 52.28, 159.64); starts: area Cine_G10_Cutscene1; texts 587, 588, 975, 976, 977, 978, 979, 980, 981, 982, 983, 984, 985, 986, 591; likely; speakers: Charon, BattleGoddess, Swashbuckler, IslandWarrior (window: Charon) — check: line order, trigger spot, speaker <!-- rebuilt/myth-g10-charon-587 -->
- [ ] **XX_Cinematic Start G11** `myth-g11-hades-592` — pf 2320 (360.62, 22.79, 331.45); starts: area Cine_G11_Cutscene1; texts 592, 593, 987, 988, 594; likely; speakers: Hades, BattleGoddess (window: XX_Cinematic Start G11) — check: line order, trigger spot, speaker <!-- rebuilt/myth-g11-hades-592 -->
- [ ] **XX_Cinematic Start G12** `myth-g12-party-598` — pf 2320 (384.56, 24.75, 354.75); starts: after defeating Hades; texts 598, 599, 600, 601, 602, 603, 604, 605, 989, 606, 990, 991, 607, 992, 619, 608; guess; speakers: Zeus, Hades, BattleGoddess, Hades cry (window: XX_Cinematic Start G12) — check: line order, trigger spot, speaker <!-- rebuilt/myth-g12-party-598 -->

</details>

<details><summary>Dinosaur Rise (0/6)</summary>

- [ ] **Pilot** `dino-d01-crash-620` — pf 6200 (145.86, 64, 46.34); starts: area Cine_D01_Cutscene1; after the cinematic dino-intro; texts 620, 622, 623, 624, 993, 994, 626, 795, 628, 995, 627, 796, 621; likely; speakers: Explorer, VGG, Gamer, Reach, Sniff, Small Dinosaur screech, Taunt, Scared (window: Pilot) — check: line order, trigger spot, speaker <!-- rebuilt/dino-d01-crash-620 -->
- [ ] **Paleontologist** `dino-d02-dino-tracker-635` — pf 6200 (198.33, 70.12, 180.03); starts: area Cine_D02_Cutscene1; area Cine_D02_Cutscene2; texts 635, 636, 637, 1010, 638, 1011, 641, 642, 643, 1012, 645, 652, 646; likely; speakers: Dino Tracker, Swashbuckler, Adventurers (window: Paleontologist) — check: line order, trigger spot, speaker <!-- rebuilt/dino-d02-dino-tracker-635 -->
- [ ] **Paleontologist** `dino-d03-expedition-694` — pf 6200 (198.33, 70.12, 180.03); starts: area Cine_D02_Cutscene1; area Cine_D02_Cutscene2 (needs tags 910; goals GBWD); texts 694, 691, 798, 692, 693, 695; guess; speakers: Dino Tracker, Swashbuckler (window: Paleontologist) — check: line order, trigger spot, speaker <!-- rebuilt/dino-d03-expedition-694 -->
- [ ] **Deep Sea Diver** `dino-springs-scuba-diver-669` — pf 6200 (249.22, 37.02, 80.27); starts: area Cine_DQ1_Cutscene1; texts 669, 670, 996, 997, 998, 999, 671, 1000, 672, 673; likely; speakers: Scuba Diver, Wizard, Jester (window: Deep Sea Diver) — check: line order, trigger spot, speaker <!-- rebuilt/dino-springs-scuba-diver-669 -->
- [ ] **Caveman** `dino-volcano-caveman-674` — pf 6200 (107.35, 68.69, 186.16); starts: area Cine_DQ2_Cutscene1; area Cine_DQ2_Cutscene2; area Cine_DQ2_Cutscene3; texts 674, 675, 1001, 676, 677, 1002, 1003, 1004, 1005, 678; likely; speakers: Caveman, BattleGoddess, Elementius, Idle pose (window: Caveman) — check: line order, trigger spot, speaker <!-- rebuilt/dino-volcano-caveman-674 -->
- [ ] **Zookeeper** `dino-tar-pits-zookeeper-679` — pf 6200 (285.67, 72.31, 130.58); starts: area Cine_DQ3_Cutscene1; area Cine_DQ3_Cutscene2; texts 679, 680, 1006, 681, 1007, 682, 1008, 683, 1009, 684; likely; speakers: Zookeeper, Miner (window: Zookeeper) — check: line order, trigger spot, speaker <!-- rebuilt/dino-tar-pits-zookeeper-679 -->

</details>

<details><summary>Gauntlet (0/3)</summary>

- [ ] **XX_Cinematic Start D04** `dino-d04-t-rex-653` — pf 6300 (448.35, 58.4, 210.19); starts: area Cine_D04_Cutscene1; texts 653, 655, 657, 658, 659, 660, 661; likely; speakers: T-Rex (window: XX_Cinematic Start D04) — check: line order, trigger spot, speaker <!-- rebuilt/dino-d04-t-rex-653 -->
- [ ] **XX_Cinematic Start D05** `dino-d05-t-rex-defeated-662` — pf 6300 (472.07, 59, 232.98); starts: after defeating T-Rex; texts 662, 663, 664, 1013, 1014, 665, 1015, 666, 1016, 667, 1017, 668; guess; speakers: T-Rex, DinoTracker, Miner, Swashbuckler, Wizard (window: XX_Cinematic Start D05) — check: line order, trigger spot, speaker <!-- rebuilt/dino-d05-t-rex-defeated-662 -->
- [ ] **Video Game Guy** `dino-d06-party-685` — pf 6300 (341.16, 61.1, 208.46); starts: area Cine_D06_Cutscene1; area Cine_D06_CutsceneFallback1 (needs tags 1540); texts 685, 1018, 1019, 1020, 1021, 1022, 1023, 1024, 686, 690; likely; speakers: Gamer, Miner, Wizard, Swashbuckler, DinoTracker (window: Video Game Guy) — check: line order, trigger spot, speaker <!-- rebuilt/dino-d06-party-685 -->

</details>
