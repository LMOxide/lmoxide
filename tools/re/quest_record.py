"""Decode QuestFullUpdateIIR / MainQuestUpdateIIR_c bodies (big-endian), after the client's
writers (Linux build): QuestFullUpdateIIR::WriteSubClass, quests::operator<<(Quest),
quests::QuestGoal::WriteToStream, MainQuestUpdateIIR_c::WriteSubClass.

QuestFullUpdateIIR: u32 1, u32 owner (character instance), Quest, u8.
Quest: u32 1, u32 template, u32, u32, u32, u32, u32, u32 main quest, u32, u8, ID32 x2 (u32
type, u32 instance), u32, u32 time left, u64, u32, 2 media lists, u32, u64, 2 media lists (u32 n
+ n x 64 bytes), goals (u32 n + n x QuestGoal).
QuestGoal: u32 1, u32 goal type, 7 x u32 (template, quest, ...), 3 x ID32, 6 x u32, 3 x u8,
string, string, u32 time left, then the goal type's own data.
MainQuestUpdateIIR_c: u32, u32 main quest, u32 current quest, u32 (n + 1) * 0x3f1, n x (u32
quest, u8 order), string (localized name), u32, u32, u32.

  tools/re/py quest_record.py <dir with recs.pkl>     decode the recorded quest records"""
import pickle, struct, sys


class R:
    def __init__(self, b):
        self.b, self.o = b, 0

    def u32(self):
        v = struct.unpack_from(">I", self.b, self.o)[0]
        self.o += 4
        return v

    def f32(self):
        v = struct.unpack_from(">f", self.b, self.o)[0]
        self.o += 4
        return v

    def u64(self):
        v = struct.unpack_from(">Q", self.b, self.o)[0]
        self.o += 8
        return v

    def u8(self):
        v = self.b[self.o]
        self.o += 1
        return v

    def string(self):
        n = struct.unpack_from(">H", self.b, self.o)[0]
        s = self.b[self.o + 2 : self.o + 2 + n]
        self.o += 2 + n
        return s.rstrip(b"\0").decode("latin-1")

    def rest(self):
        return self.b[self.o :]


def media_list(r):
    n = r.u32()
    out = [r.b[r.o + 64 * i : r.o + 64 * (i + 1)].hex() for i in range(n)]
    r.o += 64 * n
    return out


def goal(r):
    g = {"marker": r.u32(), "type": r.u32(), "u": [r.u32() for _ in range(7)]}
    g["ids"] = [(r.u32(), r.u32()) for _ in range(3)]
    g["v"] = [r.u32() for _ in range(6)]
    g["flags"] = [r.u8() for _ in range(3)]
    g["s1"], g["s2"] = r.string(), r.string()
    g["time_left"] = r.u32()
    return g


def quest(r):
    q = {"marker": r.u32(), "template": r.u32(), "a": [r.u32() for _ in range(5)], "main_quest": r.u32(), "b": r.u32(), "flag": r.u8()}
    q["ids"] = [(r.u32(), r.u32()), (r.u32(), r.u32())]
    q["c"], q["time_left"], q["u64"], q["d"] = r.u32(), r.u32(), r.u64(), r.u32()
    q["media1"], q["media2"] = media_list(r), media_list(r)
    q["e"], q["u64b"] = r.u32(), r.u64()
    q["media3"], q["media4"] = media_list(r), media_list(r)
    return q


def full_update(b):
    r = R(b)
    out = {"marker": r.u32(), "owner": r.u32()}
    out["quest"] = quest(r)
    out["rest"] = r.rest().hex()
    return out


def main_quest_update(b):
    r = R(b)
    out = {"a": r.u32(), "main_quest": r.u32(), "current": r.u32()}
    n = r.u32() // 0x3F1 - 1
    out["quests"] = [(r.u32(), r.u8()) for _ in range(n)]
    out["name"] = r.string()
    out["tail"] = [r.u32() for _ in range(3)]
    return out


if __name__ == "__main__":
    recs = pickle.load(open(sys.argv[1] + "/recs.pkl", "rb"))
    for n, x in enumerate(recs):
        if x["name"] == "QuestFullUpdateIIR":
            print(n, full_update(x["body"]))
        elif x["name"] == "MainQuestUpdateIIR_c":
            print(n, main_quest_update(x["body"]))
