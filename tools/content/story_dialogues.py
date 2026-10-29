#!/usr/bin/env python3
"""Rebuilt story conversations: the client's dialogue texts (TDB 50002) that no recording shows,
grouped into conversations and given a start (research/lmoxide-notes/client-survey/follow-ups.md
"More decisions": rebuild them, log every guess). Rerunnable; writes

  - content/dialogues/<slug>-<first text>.json (source "guess", tags ["reconstructed",
    <confidence>], `comment` = the evidence),
  - content/texts.json `answers`: the answer button (TDB 50001) that leads to each rebuilt text,
    where none was known (see `answers()`),

and gives tools/content/import_story.py the triggers (`TRIGGERS`: area -> dialogues) and the
beats they replace. Run import_story.py afterwards (it imports this module and runs `build`), then
`cargo run -p lmoxide-content -- fmt`.

How a conversation was rebuilt (the method, calibrated on the user's live footage of the Jester
at the Ruined Fort: texts 283, 284, 285 at about (148.8, 42.1, 115.1)):

  1. Texts: a start text (`changeguivisibility 0`, `addcinematicactor` its actors at Cine_*
     markers), the lines that use those actors or the same speakers, and its epilogue (no spoken
     line, `clearcinematic`, the server hooks: addtagtoplayer = the lore "Cinematic Flag" of the
     beat, resolvegoal, spawn). The epilogue sits next to the start in the TDB (before it in the
     Medieval texts: 282 before 283, 287 before 288; after it elsewhere) and often removes the
     start's actors (282 `removecinematicactor Dummy`). Lines added later have higher ids (the
     849-1024 block); they are placed by their actors, speakers and wording (narrative order).
  2. Place: the playfield that has all of its markers; the start is the story area nearest them
     there (an unmatched BA_Cine_* / Cine_* area or a silent beat's area), or the defeat of the
     boss it follows ("I surrender, I'm beaten"), or the Video Game Guy's hub area. The client's
     NPC names "Cinematic Start <lore flag>" (TDB 51000) are the knubot owners live used: they
     name the conversations (the window's npc name).
  3. Order of play: the dialogue runs as knubot.rs runs the recorded ones (each tag in order, the
     next text's answer button, the epilogue on goodbye).

Jester check: area BA_Cine_Placeholder1 (5300) is 5.6 m from Cine_DummySpawn1, the only marker
of 283; 282 is that scene's epilogue (it removes 283's actor Dummy, plays 283's poof 245460 on
InvisDummy, gives 430 The Realm of Jest and resolves VB6T "Find the Jester at the Ruined Fort"),
so it runs after 285, on goodbye.

  python3 tools/content/story_dialogues.py [content dir]     (needs the client for TDB 50001)
"""
import difflib
import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
CONTENT = sys.argv[1] if __name__ == "__main__" and len(sys.argv) > 1 else os.path.join(HERE, "..", "..", "content")
ME = "tools/content/story_dialogues.py"

# (slug, title, npc_name (TDB 51000), npc (the speaking NPC's spawn name, "" = the player: a
#  cinematic), playfield, texts in play order (the epilogue last), start, conditions,
#  confidence, evidence)
# start: ("area", [area slugs]) | ("boss", npc name or "boss") | ("after", first text)
# conditions: requires (tags), unless (tags), goals (story goals resolved)
L, G = "likely", "guess"
CONV = [
    # ------------------------------------------------------------------ Pirate World
    ("rescue-revolutionary-soldier", "Cinematic Rescue Revolutionary Soldier", 2683, "", 1632,
     [778, 780, 849, 850, 851, 852, 853, 782, 779], ("area", ["cine-rescue-revolutionary-soulder1"]), {}, L,
     "as the live Island Warrior rescue (773, area Cine_Rescue_Island_Warrior1): area Cine_Rescue_Revolutionary_Soulder1 "
     "13.8 m from its 10 markers (1632); epilogue 779 gives 4142 Rescue Revolutionary Soldier, resolves 0JSC (the "
     "Pirate line's step after FBRB, capture order); 849-853 use its actors (RevoSoldier, Swashbuckler, IslandWarrior)"),
    ("rescue-pizza-guy", "Cinematic Rescue Pizza Guy", 2684, "", 1670,
     [783, 784, 785, 786, 860, 861, 862, 787, 788], ("area", ["cine-rescue-pizza-guy1"]), {}, L,
     "area Cine_Rescue_Pizza_Guy1 14.3 m from its 11 markers (1670); epilogue 788 gives 4143, resolves CFBT (next "
     "Pirate step); 860 answers 786 (\"didn't tip me last time?\" - \"No...that was someone else\")"),
    ("pizza-delivery-to-fort", "Cinematic Start Pizza Delivery To Fort", 2721, "", 4200,
     [811, 857, 808, 807, 858, 859, 809, 810], ("area", ["cine-pizzadeliverytrigger1"]), {"requires": [4143]}, L,
     "the live capture spawned 'Cinematic Start Pizza Delivery To Fort' on Isle of Yarr; area Cine_PizzaDeliveryTrigger1 "
     "4.2 m from its 17 markers; speaker label 'With Pizza Guy' -> needs 4143 (Pizza Guy rescued); epilogue 810 gives "
     "4161 Stop at the Fort Gate, resolves H1DR; 857-859 (Pizza Guy, 'even the bad guys tip') use its actors"),
    ("pizza-delivery-to-fort-turned-away", "Cinematic Start Pizza Delivery To Fort", 2721, "", 4200,
     [801, 805, 803, 804, 802], ("area", ["cine-pizzadeliverytrigger1"]), {"unless": [4143]}, G,
     "the same gate 'Without Pizza Guy' (speaker labels of 801/802): the guard turns the crew away; epilogue 802 "
     "gives nothing; 805/806 and 803/856, 804/855 are duplicates (806, 855, 856 left out)"),
    ("dreadleg-escapes-again", "Cinematic Start Dreadleg Escapes Again", 2728, "", 4200,
     [828, 829, 830, 831, 833, 841, 834, 836, 840], ("area", ["cinematic-dreadleg1"]), {"requires": [4161]}, L,
     "area Cinematic_Dreadleg1 19.9 m from its 11 markers (Cine_*AfterFight*); epilogue 840 gives 4183 Dreadleg "
     "Escapes Again, resolves AKJH (Pirate step after H1DR) and teleports; needs 4161 (inside the fort)"),
    ("dreadleg-appears", "Cinematic Start Dreadleg Appears", 1668, "", 4200,
     [382, 383, 384, 385], ("area", ["ba-cine-dreadlegappears1"]), {}, L,
     "was the silent beat 385 on BA_Cine_DreadlegAppears1 (3.6 m from Cine_Dreadleg1); 541 Dreadleg Appears, K0TT"),
    ("welcome-to-pirate-town", "Cinematic Start Welcome to Pirate Town", 1669, "", 4200,
     [386, 387, 843, 845, 863, 864, 865, 846, 394], ("area", ["ba-cine-welcometopiratetown1"]), {}, L,
     "was the silent beat 394 on BA_Cine_WelcomeToPirateTown1 (2 m from its 7 markers; the live capture spawned "
     "'Cinematic Start Welcome to Pirate Town' here); 843/845/846 (council, 'this way!') and 863-865 (Revo, Island "
     "Warrior: 'aren't you a pirate captain?') use its actors; 542, KSKL"),
    ("the-pegleg-mermaid", "Cinematic Start The Pegleg Mermaid", 982, "", 4620,
     [155, 157, 866, 867, 868, 869, 166, 167, 870, 380, 381, 170, 171, 172, 173, 174, 175, 177, 156],
     ("area", ["ba-cine-thepeglegmermaid1"]), {}, L,
     "was the silent beat 156 on BA_Cine_ThePeglegMermaid1 (4.8 m from 11 of its 12 markers); Pete's actor 'Pete' "
     "links 866-870 ('is there anybody you don't owe money?' - 'Err...Dreadleg?' - 'Don't ye be mentionin' that "
     "scallywag') and 380/381 (THE PIRATE TRIALS!); 176 duplicates 173 (left out); 310, ATSN"),
    ("now-we-must-pirate-fight", "Cinematic Start Now we must pirate fight", 991, "", 4300,
     [193, 195, 194, 197, 199], ("area", ["ba-cine-nowwemustpiratefight"]), {}, L,
     "area BA_Cine_NowWeMustPirateFight 2.3 m from its markers; 311 'Now we must pirate fight!' (its epilogue 199 "
     "sets no tag: played once)"),
    ("completed-the-trials", "Cinematic Start Completed The Trials", 1233, "", 4300,
     [263, 395, 264, 265, 396, 397, 398, 399, 400, 401, 403, 402, 871, 266], ("boss", "Captain Pete"), {}, G,
     "Pete: 'Ye win!' - after beating him in the trials (Cine_PeteWin1 16.5 m from the fight's area); epilogue 266 "
     "resolves Y0U7 'Complete the Pirate Trials', gives 312 and plays BecomeAPirate; 395-403 and 871 use its actors; "
     "started when Captain Pete is defeated in the Pirate Trials (guess)"),
    ("dreadleg-volcano", "Cinematic Start Dreadleg Volcano", 1673, "", 4210,
     [404, 405, 406, 407, 408], ("area", ["ba-cine-dreadlegvolcano1"]), {}, L,
     "was the silent beat 408 on BA_Cine_DreadlegVolcano1; 439, 5YHB"),
    ("into-the-skull", "Cinematic Start Into the Skull", 1011, "", 4310,
     [200, 201, 203, 204], ("area", ["ba-cine-intotheskull1"]), {}, L,
     "was the silent beat 204 on BA_Cine_IntoTheSkull1 (2.8 m from Cine_DreadlegAppears1); 315, 9HSH"),
    ("captain-dreadleg", "Cinematic Start Captain Dreadleg", 965, "", 4310,
     [127, 128, 131, 132, 134], ("area", ["ba-cine-dreadleg1"]), {}, L,
     "area BA_Cine_Dreadleg1 3.8 m from its markers (Cine_PlayerAtPreBoss1: before the boss); 316 Captain Dreadleg"),
    ("dreadleg-defeated", "Cinematic Start Dreadleg Defeated", 1219, "", 4310,
     [259, 260, 261, 873, 872, 874, 875, 876, 877, 878, 879, 262], ("boss", "Captain Dreadleg"), {"requires": [316]}, G,
     "Swashbuckler: 'Stand down, Dreadleg. You're beaten!' - after the boss (markers 43 m past BA_Cine_Dreadleg1); "
     "872-879 use its actors (the curse breaks, Smith is back); epilogue 262 gives 423 The First Keystone, resolves "
     "WZ4U; the content's proposed Captain Dreadleg stands in 4210, not here, so this waits for a boss in 4310"),
    ("wizard-calls-the-compass", "Cinematic Start Dreadleg Defeated", 1219, "", 4310,
     [880, 881, 882, 883, 884, 885, 886], ("after", 259), {}, G,
     "the Wizard's failed summoning spell right after Dreadleg's defeat (its actors at the same spot; Swashbuckler: "
     "'to get to either of them, we need to go through LEGO Center'); no epilogue of its own"),
    # ------------------------------------------------------------------ Medieval World
    ("medieval-world-news", "Video Game Guy", 1725, "Video Game Guy", 1200,
     [267, 268, 468, 270, 269, 464, 271], ("area", ["ba-cine-gamerquestgiver1"]), {"requires": [316], "unless": [437]}, L,
     "the Video Game Guy at the LEGO Center (BA_Cine_GamerQuestgiver1 is 3 m from his live spawn): 'Hope you had fun "
     "in the Pirate World!'; epilogue 271 '[Medieval Quest]{spawn KFTR}' starts the Medieval story line; after "
     "Pirate World (316, as the after-pirate-world cinematic it follows), before Medieval World is done (437)"),
    ("the-kings-court", "Cinematic Start The Kings Court", 1344, "", 5200,
     [272, 273, 274, 275, 416, 417, 418, 419, 276],
     ("area", ["ba-cine-thekingscourt1", "ba-story-teleporttothroneroom-cinematicfailed"]), {}, L,
     "after the Medieval intro cinematic (then_dialogue: the arrival point is in BA_Story_AutoTeleportToThroneRoom1, "
     "whose 'teleport to the throne room' is 272's {moveactortopos playerActor TP_ThroneRoom}); also on "
     "BA_Cine_TheKingsCourt1 (9.1 m from its 3 markers) and BA_Story_TeleportToThroneRoom_CinematicFailed (when the "
     "intro was seen); 416-419 (the Wizard answers the King) use its actors; epilogue 276 gives 425, resolves CPIJ "
     "'Go to the Kingdom', spawns 8G4X (the Wizard's minifigure bag: Lua 4048 'XX_give wizard bag tag' sets 4242 "
     "Received Wizard) and teleports to the Kingdom's arrival point (88.3, 24.5, 256)"),
    ("kingnapped", "Knight", 1342, "Knight", 5200,
     [278, 280, 281, 420, 421, 422, 423, 424, 425, 277], ("area", ["ba-cine-cityguards1"]), {"requires": [425]}, L,
     "area BA_Cine_CityGuards1 1.7 m from Cine_KnightSpawn2 and the Knight spawn; Knight1 is the owner, Knight2 a "
     "client actor; epilogue 277 (before 278, as the Medieval texts put them) gives 426 Kingnapped!, resolves 24IC"),
    ("sir-loins-checkpoint", "Sir Loin", 1716, "Sir Loin", 5200,
     [426, 427, 887, 428, 429, 430, 431, 432, 433, 434], ("area", ["story-wallcheckpoint1"]), {"requires": [426]}, L,
     "Story_WallCheckpoint1 is 3 m from its 7 markers and Sir Loin's spawn (it resolved 24IC by name before: that goal "
     "is the Knights' epilogue 277); 887 answers 427; epilogue 434 gives 561 Sir Loins Checkpoint, resolves KM38"),
    ("the-realm-of-jest", "Cinematic Start The Realm of Jest", 1234, "", 5300,
     [283, 284, 285, 282], ("area", ["ba-cine-placeholder1"]), {}, L,
     "the calibration (user's live footage: the Jester says 283, 284, 285 at about (148.8, 42.1, 115.1)): area "
     "BA_Cine_Placeholder1 5.6 m from Cine_DummySpawn1; epilogue 282 (removes 283's Dummy) gives 430, resolves VB6T"),
    ("working-for-the-enemy", "Cinematic Start Working for the Enemy", 1718, "", 5300,
     [436, 437, 439, 440, 441, 442, 443, 444, 445, 446], ("area", ["ba-cine-workingfortheenemy1"]), {}, L,
     "was the silent beat 446 on BA_Cine_WorkingForTheEnemy1; 562"),
    ("my-kingdom-for-a-crown", "Cinematic Start My Kingdom for a Crown", 1237, "", 5300,
     [288, 289, 291, 447, 888, 448, 449, 450, 889, 890, 891, 451, 892, 893, 452, 287], ("boss", "boss"),
     {"requires": [562]}, G,
     "the Jester: 'I surrender, I'm beaten' - after the Ruined Fort's boss (its markers 25 m past the boss arena's "
     "BA_Cine_WorkingForTheEnemy1); 447-452 and 888-893 use its actors; epilogue 287 gives 431, spawns GQKS (the "
     "Jester joins), resolves I502"),
    ("the-faerie-queen", "Fairy Queen", 0, "Fairy Queen", 5210,
     [293, 294, 295, 296, 453, 895, 894, 454, 292], ("area", ["ba-cine-thefaeriequeen1"]), {}, L,
     "was the silent beat 292 on BA_Cine_TheFaerieQueen1 (2.3 m from its 4 markers and the Fairy Queen's spawn); "
     "speaker label 'BEFORE Quest is done'; 433, OEA3"),
    ("crossing-the-barrier", "Cinematic Start Crossing the Barrier", 1248, "Fairy Queen", 5210,
     [314, 316, 896, 317, 318, 319, 320, 321], ("area", ["ba-cine-thefaeriequeen1"]),
     {"requires": [433], "goals": ["3UX1"]}, L,
     "the same place, speaker label 'AFTER Quest is done' (the power crystals, goal 3UX1); 896 ('Beautiful!') answers "
     "316; epilogue 321 gives 434 Crossing the Barrier, resolves XOAJ"),
    ("defeated-elementius", "Cinematic Start Defeated Elementius", 1363, "", 5310,
     [308, 310, 311, 312, 313, 457, 458, 459, 460, 461, 462, 463, 897, 898, 899, 900, 309],
     ("area", ["ba-cine-placeholder1"]), {}, G,
     "BA_Cine_Placeholder1 of the Sorcerer's Keep is 19.5 m from all 5 of its markers, past the boss (Evil Sorcerer at "
     "(329, 309)); 'Surrender Elementius, you are beaten!'; 457-463 and 897-900 use its actors (the King arrives); "
     "epilogue 309 gives 437 Another Cube Piece, resolves 4H62"),
    # ------------------------------------------------------------------ Space World
    ("space-world-news", "Video Game Guy", 1725, "Video Game Guy", 1200,
     [466, 465, 469, 467], ("area", ["ba-cine-gamerquestgiver1"]), {"requires": [437], "unless": [492]}, L,
     "the Video Game Guy at the LEGO Center (his area, Cine_SpaceMiner1 3.8 m away): the Space Miner's distress call; "
     "epilogue 467 '[Space Quest]{spawn XQ65}' starts the Space story line; after Medieval World (437)"),
    ("space-s02-briefing", "Cinematic Start Space S03", 1729, "", 3200,
     [327, 329, 486, 328], ("area", ["ba-cine-s02"]), {}, L,
     "area BA_Cine_S02 19 m from its 12 markers (the briefing soldiers); epilogue 328 gives 483 XX_S2, resolves PIJN"),
    ("space-s03-mean-soldier", "Cinematic Start Space S04", 1732, "", 3200,
     [487, 488, 491, 529, 901, 489], ("area", ["ba-cine-s03-meansoldier1"]), {}, G,
     "area BA_Cine_S03_MeanSoldier1 1.8 m from its markers; 491/529/901 (the alien scares him off: his runaway "
     "marker) placed before the epilogue 489 (484 XX_S3, spawns 5PJB) - their order is a guess"),
    ("space-hq-preboss", "Cinematic Start Space S05", 1731, "", 3300,
     [330, 332, 492, 493, 494, 523, 331], ("area", ["ba-cine-preboss-fight1"]), {}, L,
     "area 'BA_Cine_Preboss Fight1' 26 m from its markers in Space Command HQ; the Queen and Shadrack (492-494, 523 "
     "use their actors); epilogue 331 gives 485 XX_S4"),
    ("space-hq-thrace", "XX_Cinematic Start S3.5", 1608, "", 3300,
     [363, 362, 902, 364], ("boss", "boss"), {"requires": [485]}, G,
     "Thrace: 'you sure helped us out' - after the HQ's boss; 362 uses its actor Sergeant_Thrace; epilogue 364 gives "
     "501 XX_S3.5, resolves MS41"),
    ("space-mercs-at-mines", "Cinematic Start Space S06", 1730, "", 3200,
     [495, 497, 903, 496], ("area", ["ba-cine-mercsatmines1"]), {}, L,
     "area BA_Cine_MercsAtMines1 15.6 m from its markers; 585 XX_MercsAtTheMines"),
    ("space-meet-the-space-miner", "XX_Cinematic Start S4", 1601, "", 3200,
     [334, 333, 498, 499, 904, 905, 906, 908, 909, 910, 335], ("area", ["ba-cine-s07"]), {}, L,
     "area BA_Cine_S07 7.5 m from its markers; 498-499 and 904-910 (the Pizza Guy 'spy', sharing the crystals) use its "
     "actors; epilogue 335 gives 587 XX_S07_MeetTheSpaceMiner, resolves 37G8"),
    ("space-help-fear", "XX_Cinematic Start S5", 1602, "", 3200,
     [337, 338, 911, 912, 913, 500, 501, 514, 907, 336], ("area", ["ba-cine-s08"]), {}, L,
     "area BA_Cine_S08 1.8 m from its markers; the odds of the crystal raid (911-913, 500, 501, 514) use its actors; "
     "epilogue 336 gives 588 XX_S08_Help_F.E.A.R, resolves VJMG"),
    ("space-science-facility", "XX_Cinematic Start S6", 1603, "", 3210,
     [361, 359, 502, 914, 915, 916, 917, 918, 360], ("area", ["ba-cine-s09"]), {}, L,
     "area BA_Cine_S09 2.3 m from its 7 markers (next to where 341 lands the player); the compass finds the alien "
     "signal (914-918); epilogue 360 gives 488 XX_S7, resolves 3XT2"),
    ("space-black-hole", "XX_Cinematic Start S8", 1511, "", 3210,
     [343, 344, 369, 370, 371, 924, 925, 926, 927, 928, 929, 930, 931, 932, 933, 934, 342],
     ("area", ["ba-cine-s10"]), {}, L,
     "area BA_Cine_S10 5.9 m from its markers; the alien trooper (927-934, Cine_HoleAlien1 = 342's marker); epilogue "
     "342 gives 489 XX_S8, spawns FXMI, resolves UC8W"),
    ("space-builder-ruins", "Cinematic Start S09.5", 2781, "", 3210,
     [919, 921, 922, 923, 920], ("area", ["ba-cine-s095"]), {}, L,
     "area BA_Cine_S095 1.6 m from its markers; 4243 XX_S09.5_AtTheBuilderRuins"),
    ("space-shadrack", "Cinematic Start S11", 1737, "", 3310,
     [503, 504, 935, 936, 937, 938, 939, 505], ("area", ["ba-cine-s11"]), {}, L,
     "area BA_Cine_S11 3.1 m from its markers; 592 XX_S11_Shadrack_BossFight"),
    ("space-victory-over-shadrack", "Cinematic Start S12", 1738, "", 3310,
     [506, 507, 508, 940, 941, 509], ("area", ["ba-cine-s12"]), {"requires": [592]}, L,
     "area BA_Cine_S12 2.6 m from its markers (after the Shadrack fight: needs 592); 593 XX_S12VictoryoverShadrack"),
    ("space-alien-queen", "Cinematic Start S13", 1513, "", 3310,
     [349, 350, 510, 942, 511, 943, 512, 513, 522, 944, 348], ("area", ["ba-cine-s13"]), {}, L,
     "area BA_Cine_S13 4.5 m from its 7 markers; 942-944 (the Space Miner bargains) use its actors; epilogue 348 "
     "gives 491 The Alien Queen, resolves 1RYD"),
    ("space-the-queen-falls", "Cinematic Start S14", 1514, "", 3310,
     [353, 351, 945, 946, 376, 377, 515, 516, 518, 519, 520, 949, 521, 947, 948, 950, 352], ("boss", "Alien Queen"),
     {"requires": [491]}, G,
     "'You smashed the Queen and saved the day all by yourself?' - after the Alien Queen (its 13 markers 30 m from "
     "BA_Cine_S13); 945-950 and 515-521 use its actors (order by wording); epilogue 352 gives 492 XX_S11, spawns 7WZE, "
     "resolves HMGJ"),
    # ------------------------------------------------------------------ Mythology World
    ("mythology-world-news", "Video Game Guy", 1725, "Video Game Guy", 1200,
     [530, 531, 533, 532, 576], ("area", ["ba-cine-gamerquestgiver1"]), {"requires": [492], "unless": [676]}, L,
     "the Video Game Guy at the LEGO Center: 'Did you have fun in Space?' and the stone tablet; epilogue 576 "
     "'[Mythology Quest]{spawn U3FC}' starts the Mythology story line; after Space World (492)"),
    ("myth-g01-zeus", "Zeus", 1821, "Zeus", 2210,
     [572, 573, 951, 574, 953, 575], ("area", ["cine-g01-cutscene1"]), {}, L,
     "was the silent beat 575 on Cine_G01_Cutscene1 (Zeus's live spawn is in it; he speaks as the owner); 951/953 "
     "(the toga) are Zeus's; 665 XX_G01, FEYF"),
    ("myth-g02-party-guest", "Commoner", 1825, "Commoner", 2210,
     [577, 578, 579, 580], ("area", ["cine-g02-cutscene1"]), {}, L,
     "was the silent beat 580 on Cine_G02_Cutscene1 (16 m from its 6 markers); the party guest (owner) is the "
     "nearest Commoner; 666 XX_G02, spawns RQJT"),
    ("myth-g03-grey-sisters", "XX_Cinematic Start G03", 1827, "", 2210,
     [534, 535, 536, 537, 538, 539, 540, 541, 542, 957, 954, 958, 955, 959, 545], ("area", ["cine-g03-cutscene1"]), {}, L,
     "was the silent beat 545 on Cine_G03_Cutscene1 (1.3 m from its markers); 954-959 ('a toga in your future', the "
     "Wizard's 'But-') use its actors (956 duplicates 954); 667 XX_G03, 0AHO"),
    ("myth-g04-medusa", "XX_Cinematic Start G04", 1829, "", 2300,
     [595, 615, 596, 597], ("area", ["cine-g04-cutscene1"]), {}, L,
     "was the silent beat 597 on Cine_G04_Cutscene1 (1 m from its markers); 615 uses Medusa; 668 XX_G04"),
    ("myth-g05-battle-goddess", "XX_Cinematic Start G05", 1830, "", 2300,
     [546, 547, 548, 962, 963, 550, 964, 551, 552, 553, 554, 965, 617, 966, 555], ("boss", "Medusa"), {}, G,
     "Battle Goddess freed from stone - after Medusa (no area: its 9 markers are 25 m from G04's); 962-966 and 617 use "
     "its actors; epilogue 555 gives 669 XX_G05, resolves 59MF, spawns 9UO2"),
    ("myth-g06-puzzle-lock", "XX_Cinematic Start G06", 1833, "", 2310,
     [556, 557, 558, 559], ("area", ["cine-g06-cutscene1"]), {}, L,
     "was the silent beat 559 on Cine_G06_Cutscene1; 670 XX_G06"),
    ("myth-g07-thunderbolt", "XX_Cinematic Start G07", 1835, "", 2310,
     [560, 561, 562, 563, 564, 565, 566, 567], ("area", ["cine-g07-cutscene1"]), {}, L,
     "was the silent beat 567 on Cine_G07_Cutscene1; 671 XX_G07"),
    ("myth-g08-the-key", "XX_Cinematic Start G08", 1841, "", 2310,
     [568, 569, 960, 961, 570, 618, 571], ("area", ["cine-g08-cutscene1"]), {"requires": [671]}, L,
     "was the silent beat 571 on Cine_G08_Cutscene1 (now after G07's 671: Thunderbolt is beaten first); 960/961 "
     "(chicken wings) use its actors; 672 XX_G08, R1KN"),
    ("myth-g09-the-trap", "XX_Cinematic Start G09", 1844, "", 2320,
     [581, 582, 583, 972, 973, 974, 584, 585, 586], ("area", ["cine-g09-cutscene1"]), {}, L,
     "was the silent beat 586 on Cine_G09_Cutscene1; 972-974 (why betray us?) use its actors; 673 XX_G09"),
    ("myth-g10-charon", "Cinematic Start Charon", 0, "Charon", 2320,
     [587, 588, 975, 976, 977, 978, 979, 980, 981, 982, 983, 984, 985, 986, 591], ("area", ["cine-g10-cutscene1"]), {}, L,
     "was the silent beat 591 on Cine_G10_Cutscene1 (Charon's spawn 9 m away speaks as the owner); 975-986 use its "
     "actors; 674 XX_G10"),
    ("myth-g11-hades", "XX_Cinematic Start G11", 1846, "", 2320,
     [592, 593, 987, 988, 594], ("area", ["cine-g11-cutscene1"]), {}, L,
     "was the silent beat 594 on Cine_G11_Cutscene1; 675 XX_G11"),
    ("myth-g12-party", "XX_Cinematic Start G12", 1847, "", 2320,
     [598, 599, 600, 601, 602, 603, 604, 605, 989, 606, 990, 991, 607, 992, 619, 608], ("boss", "Hades"), {}, G,
     "Hades: 'Ouch! I give up!' - after Hades (its markers 33 m from G11's area); 989-992 (Zeus) use its actors; "
     "epilogue 608 gives 676 XX_G12, resolves 1DPQ"),
    # ------------------------------------------------------------------ Dino World
    ("dino-world-news", "Video Game Guy", 1725, "Video Game Guy", 1200,
     [633, 629, 630, 631, 634], ("area", ["ba-cine-gamerquestgiver1"]), {"requires": [676], "unless": [909]}, L,
     "the Video Game Guy at the LEGO Center ('[DINO WORLD]', the crystal artifacts of Dinosaur Rise); epilogue 634 "
     "'[Dino Quest]{resolvegoal 3K9Q}' starts the Dino story line; after Mythology World (676)"),
    ("dino-d01-crash", "Pilot", 1922, "Pilot", 6200,
     [620, 622, 623, 624, 993, 994, 626, 795, 628, 995, 627, 796, 621], ("area", ["cine-d01-cutscene1"]), {}, L,
     "was the silent beat 621 on Cine_D01_Cutscene1 (2 m from its markers; the Pilot's spawn is the Explorer who "
     "speaks as the owner); 993-995 and 795/796 are the Explorer's; 909 XX_D01, 6BRI, spawns 29ZU (the plane parts)"),
    ("dino-d02-dino-tracker", "Paleontologist", 2833, "Paleontologist", 6200,
     [635, 636, 637, 1010, 638, 1011, 641, 642, 643, 1012, 645, 652, 646],
     ("area", ["cine-d02-cutscene1", "cine-d02-cutscene2"]), {}, L,
     "was the silent beat 646 on Cine_D02_Cutscene1/2 (2-6 m from its 9 markers); Swashbuckler's 'Treasures?!' "
     "(1010-1012, Cine_CampSwashbuckler1) and the adventurers' 652 use its actors; 910 XX_D02, I8D5"),
    ("dino-d03-expedition", "Paleontologist", 2833, "Paleontologist", 6200,
     [694, 691, 798, 692, 693, 695], ("area", ["cine-d02-cutscene1", "cine-d02-cutscene2"]),
     {"requires": [910], "goals": ["GBWD"]}, G,
     "the same camp ('You have all the pieces already?'): after the four elemental pieces (GBWD); 691-693 and 798 use "
     "its actors; epilogue 695 gives 911 XX_D03, resolves ODCB, spawns WGPC. The older 647-651 (resolves GBWD itself) "
     "is left out"),
    ("dino-springs-scuba-diver", "Deep Sea Diver", 1927, "Deep Sea Diver", 6200,
     [669, 670, 996, 997, 998, 999, 671, 1000, 672, 673], ("area", ["cine-dq1-cutscene1"]), {}, L,
     "area Cine_DQ1_Cutscene1 3.7 m from its markers and the Deep Sea Diver's spawn (the owner); 996-1000 use its "
     "actors (Wizard, Jester); 916 XX_D_Springs_Knubot"),
    ("dino-volcano-caveman", "Caveman", 1928, "Caveman", 6200,
     [674, 675, 1001, 676, 677, 1002, 1003, 1004, 1005, 678],
     ("area", ["cine-dq2-cutscene1", "cine-dq2-cutscene2", "cine-dq2-cutscene3"]), {}, L,
     "area Cine_DQ2_Cutscene3 1.3 m from its markers and the Caveman's spawn (the owner); 1001-1005 use its actors "
     "(Battle Goddess, Elementius); 915 XX_D_Volcano_Knubot"),
    ("dino-tar-pits-zookeeper", "Zookeeper", 1929, "Zookeeper", 6200,
     [679, 680, 1006, 681, 1007, 682, 1008, 683, 1009, 684], ("area", ["cine-dq3-cutscene1", "cine-dq3-cutscene2"]), {}, L,
     "area Cine_DQ3_Cutscene2 5.1 m from its marker and the Zookeeper's spawn (the owner); 1006-1009 (the Space "
     "Miner: 'ethical tests?') use its actor; 914 XX_D_Tar_Pits_Knubot"),
    ("dino-d04-t-rex", "XX_Cinematic Start D04", 1925, "", 6300,
     [653, 655, 657, 658, 659, 660, 661], ("area", ["cine-d04-cutscene1"]), {}, L,
     "was the silent beat 661 on Cine_D04_Cutscene1 (4.2 m from its markers); 912 XX_D04"),
    ("dino-d05-t-rex-defeated", "XX_Cinematic Start D05", 1926, "", 6300,
     [662, 663, 664, 1013, 1014, 665, 1015, 666, 1016, 667, 1017, 668], ("boss", "T-Rex"), {}, G,
     "T-Rex: 'Ouch! Au! You win!' - after the T-Rex (its markers 22 m from the party's area); 1013-1017 use its "
     "actors; epilogue 668 gives 913 XX_D05, resolves 4A44"),
    ("dino-d06-party", "Video Game Guy", 2802, "Video Game Guy", 6300,
     [685, 1018, 1019, 1020, 1021, 1022, 1023, 1024, 686, 690],
     ("area", ["cine-d06-cutscene1", "cine-d06-cutscenefallback1"]), {"requires": [1540]}, L,
     "was the silent beat 690 on Cine_D06_Cutscene1/Fallback1 (after the T-Rex, achievement 1540); the Video Game "
     "Guy's spawn there is the owner; 1018-1024 (why he hired the T-Rex) use its actors; 1985 XX_D06, DanceParty"),
]

# Texts left out on purpose, for the docs: (texts, likely place, why).
UNWIRED = [
    ("137-159 (143)", "the beta hub intro", "Story Intro with Space Miner, Battle Goddess, Swashbuckler; replaced by the tutorial (1300)"),
    ("470-479", "LEGO Center (beta intro)", "same epilogue as 143 (305, 329, 404, spawn WPZW): the pre-tutorial start, superseded by the tutorial"),
    ("212-215", "beta tutorial (Torn Bags)", "Video Game Guy and the torn bags; the recorded tutorial plays other texts"),
    ("88-91, 180-191 (181)", "Isle of Yarr, Escape from the Fort", "the beta escape with Captain Pete; live used 822 (Swashbuckler, recorded) which resolves the same goal ALPQ; the beat 181 on BA_Cine_EscapeFromTheFort1 stays"),
    ("231-253 (240, 252)", "Isle of Yarr harbour", "Swashbuckler Hero 'Find your way around' / 'Need directions': beta, he refers to the beta story; BA_Cine_FindYourWayAround1 shares Pirate Town's spot"),
    ("254-258, 297-301, 323-325, 339-340, 345-346, 365-367, 372-375, 378-379", "beta placeholders", "'Placeholder Dummy' / 'our Beta Elves are hard at work' texts"),
    ("341", "Space Colony -> Planet's Dark Side", "XX_S6: only its epilogue (teleport to 3210) and placeholder lines exist; nothing to show"),
    ("302-306, 455-456 (303)", "Sorcerer's Keep?", "Danger at the Bridge (Elementius prepares the spell): its only marker Cine_DummySpawn1 is not in the Sorcerer's Keep (5310), only in 5200/5300; where it played is unknown"),
    ("524-528", "Shadow Centurion Base (BA_Cine_S14?)", "Commander Z's ship: markers Cine_Commander1/Cine_SpaceShip1 are in no playfield's data"),
    ("647-651", "Dinosaur Rise camp", "the older XX_D03 version (resolves GBWD itself); 694-695 replaces it"),
    ("967-971", "Mythology Mountains (Cine_Mermaid1)", "the Mermaid asks to come along: no epilogue, no flag, no area"),
    ("609-614", "Sky Forge?", "Stormhammer's two boss barks; no area, no flag"),
    ("696-705", "winter event", "Santa (XMAS): seasonal, needs the Holiday Spirit item check"),
    ("1028, 1033, 1034, 1041", "LEGO Center funhouse entrance", "the Monster Scientist without a ticket: the ticket check is unknown (as for 1025)"),
    ("763, 764, 771, 792-794", "tutorial", "controller/touch variants of recorded tutorial lines"),
    ("176, 806, 855, 856, 956", "-", "duplicates of 173, 805, 804, 803, 954"),
    ("307", "-", "addtagtoplayer 327 alone (not a story flag)"),
]

# Extra silent beats (texts whose epilogue is a quest start without a conversation).
BEATS = {
    "area/5200/story-spawnmedievalquest": (271, "guess: Story_SpawnMedievalQuest in the Kingdom runs text 271 "
                                                "'[Medieval Quest]{spawn KFTR}' (the Video Game Guy's epilogue) for "
                                                "whoever arrives without the Medieval story line"),
}

# Answers that no wording or alignment finds: the dev button "Next" (50001/132).
FALLBACK_ANSWER = 132


def load(p):
    with open(p) as f:
        return json.load(f)


def save(p, d):
    with open(p, "w") as f:
        json.dump(d, f, indent=2, ensure_ascii=False)
        f.write("\n")


def world_ids():
    out = {}
    for d in os.listdir(os.path.join(CONTENT, "worlds")):
        w = load(os.path.join(CONTENT, "worlds", d, "world.json"))
        out[int(d.split("-")[0])] = (w["id"], d)
    return out


def norm(s):
    s = re.sub(r"\{[^}]*\}|\[[^\]]*\]|[\"*]", "", s)
    return re.sub(r"\s+", " ", s).strip().lower()


def spoken(text):
    lines = [norm(l) for l in text.splitlines()]
    lines = [l for l in lines if l]
    return lines[-1] if lines else ""


def tdb(category):
    sys.path.insert(0, os.path.join(HERE, "..", "re"))
    import tdc  # noqa: E402  (the client's TDB; needs clients/linux)
    return tdc.texts(category)


def answers(texts, known, needed):
    """The 50001 answer for each text in `needed` that `known` lacks: the answer whose wording is
    the text's spoken line (speaker marks and tags left out), else the closest wording (>= 0.6),
    else alignment: the answers were written in the same order as the texts, so an unmatched
    text between two matched ones gets the unused answer ids between theirs, in order. Returns
    {text: (answer, how)}."""
    a = {int(k): v for k, v in tdb(50001).items()}
    by = {}
    for aid in sorted(a):
        by.setdefault(norm(a[aid]), aid)
    found = {int(k): v for k, v in known.items()}
    how = {}
    # every text's wording match (also outside the conversations: alignment needs the neighbours)
    for tid, t in texts.items():
        tid = int(tid)
        line = spoken(t)
        if tid in found or not line:
            continue
        if line in by:
            found[tid] = by[line]
            how[tid] = "wording"
    for tid in needed:
        if tid in found:
            continue
        line = spoken(texts.get(str(tid), ""))
        best = max(((difflib.SequenceMatcher(None, line, norm(s)).ratio(), aid) for aid, s in a.items()), default=(0, 0))
        if best[0] >= 0.6:
            found[tid] = best[1]
            how[tid] = f"closest wording ({best[0]:.2f})"
    used = set(found.values())
    for tid in sorted(needed):
        if tid in found:
            continue
        before = [(t, x) for t, x in found.items() if t < tid]
        after = [(t, x) for t, x in found.items() if t > tid]
        if not before or not after:
            continue
        (pt, pa), (nt, na) = max(before), min(after)
        free = [i for i in range(pa + 1, na) if i in a and i not in used]
        waiting = [t for t in sorted(needed) if pt < t < nt and t not in found]
        if free and tid in waiting and waiting.index(tid) < len(free):
            found[tid] = free[waiting.index(tid)]
            used.add(found[tid])
            how[tid] = f"aligned between {pt}->{pa} and {nt}->{na}"
    out = {}
    for tid in needed:
        if str(tid) in known:
            continue
        if tid in found:
            out[tid] = (found[tid], how.get(tid, "wording"))
        else:
            out[tid] = (FALLBACK_ANSWER, "none found: the dev button 'Next'")
    return out


def markers_pos(pf, texts, all_texts, wdir):
    """The centre of the markers the conversation's texts use, in playfield pf."""
    p = os.path.join(CONTENT, "worlds", wdir, "areas.json")
    pts = {a["name"]: a["shape"]["point"]["pos"] for a in load(p)["areas"] if "point" in a.get("shape", {})}
    names = set()
    for t in texts:
        for tag in re.findall(r"\{([^}]*)\}", all_texts.get(str(t), "")):
            w = tag.split()
            if w[0] == "addcinematicactor" and len(w) > 3:
                names.add(w[3])
            elif w[0] == "moveactortopos" and len(w) > 2:
                names.add(w[2])
            elif w[0] == "movecameratopos" and len(w) > 1:
                names.add(w[1])
    got = [pts[n] for n in names if n in pts]
    if not got:
        return None
    return [round(sum(p[i] for p in got) / len(got), 2) for i in range(3)]


def area_pos(pf, wdir, slug):
    p = os.path.join(CONTENT, "worlds", wdir, "areas.json")
    for a in load(p)["areas"]:
        if a["id"] == f"area/{pf}/{slug}":
            pts = a["shape"]["polygon"]["points"]
            h = a["shape"]["polygon"].get("heights", [0, 0])
            return [round(sum(x[0] for x in pts) / len(pts), 2), round(h[0], 2), round(sum(x[1] for x in pts) / len(pts), 2)]
    raise SystemExit(f"{ME}: no area area/{pf}/{slug}")


def dialogue_id(slug, texts):
    return f"dialogue/{slug}-{texts[0]}"


# area id -> [(dialogue id, comment)] for import_story.py
TRIGGERS = {}
for slug, _t, _n, _npc, pf, texts, start, _c, conf, why in CONV:
    if start[0] == "area":
        for a in start[1]:
            TRIGGERS.setdefault(f"area/{pf}/{a}", []).append((dialogue_id(slug, texts), f"{conf} (reconstructed): {why}"))


def build():
    """Writes the dialogue records and the answers they need."""
    tpath = os.path.join(CONTENT, "texts.json")
    tj = load(tpath)
    all_texts = tj["texts"]
    worlds = world_ids()
    needed = []
    for c in CONV:
        texts = c[5]
        last_is_epilogue = not spoken(all_texts.get(str(texts[-1]), ""))
        needed += texts[1:-1] if last_is_epilogue else texts[1:]
    found = answers(all_texts, tj["answers"], needed)
    for tid, (aid, how) in sorted(found.items()):
        tj["answers"][str(tid)] = aid
    tj["answers"] = dict(sorted(tj["answers"].items(), key=lambda kv: int(kv[0])))
    save(tpath, tj)
    ddir = os.path.join(CONTENT, "dialogues")
    for f in os.listdir(ddir):
        if "reconstructed" in load(os.path.join(ddir, f)).get("tags", []):
            os.remove(os.path.join(ddir, f))
    for i, (slug, title, npc_name, npc, pf, texts, start, cond, conf, why) in enumerate(CONV):
        wid, wdir = worlds[pf]
        did = dialogue_id(slug, texts)
        rec = {"id": did, "name": title, "texts": texts, "world": wid}
        if npc_name:
            rec["npc_name"] = npc_name
        rec["npc"] = npc or title
        if start[0] == "area":
            rec["start"] = "trigger"
            pos = area_pos(pf, wdir, start[1][0])
        elif start[0] == "boss":
            rec["start"] = "boss"
            rec["defeated"] = start[1]
            pos = markers_pos(pf, texts, all_texts, wdir)
        else:
            rec["start"] = "after_dialogue"
            rec["follows"] = start[1]
            pos = markers_pos(pf, texts, all_texts, wdir)
        if pos:
            rec["pos"] = pos
        for k in ("requires", "unless", "goals"):
            if cond.get(k):
                rec[k] = cond[k]
        rec["order"] = 100 + i
        rec["source"] = "guess"
        rec["tags"] = ["reconstructed", conf]
        notes = [f"{tid}->{aid} ({how})" for tid, (aid, how) in sorted(found.items()) if tid in texts and how != "wording"]
        rec["comment"] = f"{conf}: {why}" + (f"; answers {', '.join(notes)}" if notes else "") + f" ({ME})"
        save(os.path.join(ddir, f"{did.split('/')[1]}.json"), rec)
    fallback = [t for t, (_, how) in found.items() if how.startswith("none")]
    print(f"{len(CONV)} rebuilt dialogues, {len(found)} answers added "
          f"({sum(1 for _, h in found.values() if h == 'wording')} by wording, {len(fallback)} fallback: {fallback})")


if __name__ == "__main__":
    build()
