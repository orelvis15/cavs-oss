import { GROUPS, SECTIONS } from "../app/sections";
import { useI18n } from "../i18n";
import { useProjects } from "../app/projects";
import { Icon } from "./Icon";

export function Sidebar({
  active,
  onSelect,
}: {
  active: string;
  onSelect: (id: string) => void;
}) {
  const { section, group } = useI18n();
  const { currentProject } = useProjects();
  const engine = currentProject?.engine ?? "generic";
  const groupOrder = Object.keys(GROUPS);
  const visible = SECTIONS.filter((s) => !s.engines || s.engines.includes(engine));

  return (
    <nav className="sidebar">
      {groupOrder.map((g) => {
        const items = visible.filter((s) => s.group === g);
        if (items.length === 0) return null;
        return (
          <div key={g}>
            <div className="nav-group-label">{group(g)}</div>
            {items.map((s) => (
              <button
                key={s.id}
                className={"nav-item" + (active === s.id ? " active" : "")}
                onClick={() => onSelect(s.id)}
                title={section(s.id).label}
              >
                <Icon name={s.icon} className="nav-ico" size={17} />
                <span className="nav-label">{section(s.id).label}</span>
              </button>
            ))}
          </div>
        );
      })}
    </nav>
  );
}
